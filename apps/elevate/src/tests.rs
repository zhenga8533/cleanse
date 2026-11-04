#![allow(clippy::disallowed_methods, reason = "tests build fixture trees")]

use std::fs::{self, File};
use std::io::{BufReader, PipeReader, PipeWriter, Write, pipe};
use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use cleanse_desktop_views::Section;
use cleanse_engine::{Engine, KnownFolders, Session, System};
use tempfile::TempDir;

use crate::client::Client;
use crate::protocol::{
    self, MAX_ID_LENGTH, MAX_IDS, MAX_REQUEST, ProtocolError, Request, Response, VERSION,
};
use crate::server::{Ending, serve};

/// Writes a file dated a day ago, so it doesn't count as recent activity.
fn write_old(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_hours(24))
        .unwrap();
}

/// A helper serving a session of the user's cleanup over a project in a temporary home folder,
/// which the protocol doesn't tell apart from system cleanup's. It acts as if it isn't elevated,
/// since CI runs tests as an administrator on Windows.
struct Helper {
    root: TempDir,
    _data: TempDir,
    requests: PipeWriter,
    answers: BufReader<PipeReader>,
    serving: JoinHandle<Ending>,
}

impl Helper {
    fn start(idle: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        write_old(&home.join("web/package.json"), "{}");
        write_old(&home.join("web/node_modules/left-pad/index.js"), "abcd");
        write_old(&home.join("web/src/index.js"), "x");
        let folders = KnownFolders {
            home: Some(home),
            temp: root.path().join("temp"),
            ..KnownFolders::default()
        };
        let system = System::new()
            .unwrap()
            .elevated(false)
            .with_known_folders(folders);
        let engine = Engine::user_at(system, data.path().to_path_buf()).unwrap();
        let session = Arc::new(Session::new(engine));
        let (served, requests) = pipe().unwrap();
        let (answers, answering) = pipe().unwrap();
        let serving =
            thread::spawn(move || serve(&session, BufReader::new(served), answering, idle));
        Self {
            root,
            _data: data,
            requests,
            answers: BufReader::new(answers),
            serving,
        }
    }

    fn send_line(&mut self, line: &str) {
        self.requests.write_all(line.as_bytes()).unwrap();
        self.requests.write_all(b"\n").unwrap();
    }

    fn send(&mut self, request: &Request) {
        protocol::write(&mut self.requests, request).unwrap();
    }

    /// The next answer that isn't progress.
    fn answer(&mut self) -> Response {
        loop {
            let response = protocol::read::<Response>(&mut self.answers, usize::MAX)
                .unwrap()
                .expect("the helper stopped");
            if !response.is_progress() {
                return response;
            }
        }
    }

    fn hello(mut self) -> Self {
        self.send(&Request::Hello { version: VERSION });
        assert!(
            matches!(self.answer(), Response::Hello { version } if version == VERSION),
            "no hello back"
        );
        self
    }

    /// Scans, and returns the scan's ID.
    fn scan(&mut self) -> u64 {
        self.send(&Request::Scan {});
        match self.answer() {
            Response::Results { results } => results.scan,
            other => panic!("{other:?}"),
        }
    }

    /// Stops sending requests, and returns why the helper stopped.
    fn close(self) -> Ending {
        drop(self.requests);
        self.serving.join().unwrap()
    }

    /// Returns why the helper stopped, which it must have, without closing anything.
    fn stopped(self) -> Ending {
        self.serving.join().unwrap()
    }
}

const IDLE: Duration = Duration::from_mins(1);

#[test]
fn requests_name_no_paths_and_nothing_unknown() {
    let parse = |line: &str| serde_json::from_str::<Request>(line);

    assert_eq!(parse(r#"{"request":"scan"}"#).unwrap(), Request::Scan {});
    assert_eq!(
        parse(r#"{"request":"clean","scan":2,"items":[1,3]}"#).unwrap(),
        Request::Clean {
            scan: 2,
            items: vec![1, 3]
        }
    );
    for unknown in [
        r#"{"request":"scan","roots":["C:\\Windows"]}"#,
        r#"{"request":"clean","scan":2,"items":[1],"path":"/etc"}"#,
        r#"{"request":"delete","path":"/etc"}"#,
        r#"{"scan":2}"#,
    ] {
        assert!(parse(unknown).is_err(), "{unknown}");
    }
}

#[test]
fn requests_name_only_so_many_ids() {
    let ids = |count: usize| Request::Clean {
        scan: 1,
        items: vec![7; count],
    };
    let entries = |id: String| Request::Restore { ids: vec![id] };

    assert!(ids(MAX_IDS).check().is_ok());
    assert!(ids(MAX_IDS + 1).check().is_err());
    assert!(entries("1-2-3".into()).check().is_ok());
    assert!(entries("1".repeat(MAX_ID_LENGTH + 1)).check().is_err());
}

#[test]
fn messages_are_whole_lines_of_text_within_the_limit() {
    let read = |bytes: &[u8], max: usize| protocol::read::<Request>(&mut &bytes[..], max);

    assert_eq!(read(b"", 100).unwrap(), None);
    assert_eq!(
        read(b"{\"request\":\"scan\"}\n", 100).unwrap(),
        Some(Request::Scan {})
    );
    let line = "{\"request\":\"scan\"}";
    assert!(read(format!("{line}\n").as_bytes(), line.len()).is_ok());
    assert!(matches!(
        read(format!("{line}\n").as_bytes(), line.len() - 1),
        Err(ProtocolError::TooLong)
    ));
    assert!(matches!(
        read(b"{\"request\":\"scan\"}", 100),
        Err(ProtocolError::NotText)
    ));
    assert!(matches!(
        read(b"\xff\xfe\n", 100),
        Err(ProtocolError::NotText)
    ));
    assert!(matches!(
        read(b"scan\n", 100),
        Err(ProtocolError::Malformed(_))
    ));
}

#[test]
fn scans_and_cleans_what_the_window_names_by_id() {
    let helper = Helper::start(IDLE).hello();
    let node_modules = helper.root.path().join("home/web/node_modules");
    let mut helper = helper;
    let scan = helper.scan();
    helper.send(&Request::Candidates {
        scan,
        section: Section::Removable,
        rule: None,
        offset: 0,
        limit: 10,
    });
    let item = match helper.answer() {
        Response::Page { page } => {
            assert_eq!(page.total, 1);
            page.items[0].id
        }
        other => panic!("{other:?}"),
    };

    helper.send(&Request::Clean {
        scan,
        items: vec![item],
    });

    match helper.answer() {
        Response::Cleaned { report } => assert_eq!(report.deleted.items, 1, "{report:?}"),
        other => panic!("{other:?}"),
    }
    assert!(!node_modules.exists());
    assert_eq!(helper.close(), Ending::Closed);
}

#[test]
fn refuses_ids_from_an_earlier_scan() {
    let mut helper = Helper::start(IDLE).hello();
    let first = helper.scan();
    helper.scan();

    helper.send(&Request::Clean {
        scan: first,
        items: vec![0],
    });

    assert!(
        matches!(helper.answer(), Response::Error { message } if message.contains("earlier scan"))
    );
    assert!(helper.root.path().join("home/web/node_modules").exists());
}

/// Cancelling has no answer of its own, so the next answer is to the next request.
#[test]
fn cancelling_has_no_answer() {
    let mut helper = Helper::start(IDLE).hello();

    helper.send(&Request::Cancel {});
    helper.send(&Request::Quarantined {});

    assert!(matches!(helper.answer(), Response::Quarantine { .. }));
}

#[test]
fn the_first_request_must_be_hello_in_this_version() {
    let mut early = Helper::start(IDLE);
    early.send(&Request::Scan {});
    let mut wrong = Helper::start(IDLE);
    wrong.send(&Request::Hello {
        version: VERSION + 1,
    });

    assert!(matches!(wrong.answer(), Response::Error { .. }));
    assert!(matches!(early.stopped(), Ending::Malformed(_)));
    assert!(matches!(wrong.stopped(), Ending::Malformed(_)));
}

#[test]
fn stops_at_the_first_message_that_isnt_a_request() {
    let too_many = serde_json::to_string(&Request::Clean {
        scan: 1,
        items: vec![0; MAX_IDS + 1],
    })
    .unwrap();
    let lines = [
        "not json".to_owned(),
        r#"{"request":"scan","roots":["/"]}"#.to_owned(),
        r#"{"request":"hello","version":1}"#.to_owned(),
        too_many,
        "x".repeat(MAX_REQUEST + 1),
    ];

    for line in lines {
        let mut helper = Helper::start(IDLE).hello();
        helper.send_line(&line);

        let ending = helper.stopped();

        assert!(
            matches!(ending, Ending::Malformed(_)),
            "{}: {ending:?}",
            &line[..line.len().min(40)]
        );
    }
}

#[test]
fn stops_when_the_window_closes_its_end_or_goes_quiet() {
    let closed = Helper::start(IDLE).hello();
    let quiet = Helper::start(Duration::from_millis(200)).hello();

    assert_eq!(closed.close(), Ending::Closed);
    assert_eq!(quiet.stopped(), Ending::Idle);
}

#[test]
fn the_client_says_hello_and_asks_one_request_at_a_time() {
    let helper = Helper::start(IDLE);
    let client = Client::connect(helper.answers, helper.requests).unwrap();
    let mut progress = 0;

    let results = client
        .ask(&Request::Scan {}, &mut |_| progress += 1)
        .unwrap();
    let history = client.ask(&Request::History {}, &mut |_| {}).unwrap();

    assert!(matches!(results, Response::Results { .. }), "{results:?}");
    assert!(matches!(history, Response::History { .. }), "{history:?}");
    drop(client);
    assert_eq!(helper.serving.join().unwrap(), Ending::Closed);
}
