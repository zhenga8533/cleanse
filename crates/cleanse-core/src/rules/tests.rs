#![allow(
    clippy::disallowed_methods,
    reason = "tests build fixture trees and repositories, using the git command line"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cleanse_model::{Evidence, RuleId};
use cleanse_platform::KnownFolders;

use super::matcher::wildcard_match;
use super::{BUILTIN, Matcher, Os, Rule, RuleSet, RuleTest};

/// Known folders for fixtures: each base is a folder named after it under the test's root.
fn fixture_folders(root: &Path) -> KnownFolders {
    KnownFolders {
        home: Some(root.join("home")),
        cache: Some(root.join("cache")),
        config: Some(root.join("config")),
        data_local: Some(root.join("data_local")),
        temp: root.join("temp"),
        ..KnownFolders::default()
    }
}

fn build_tree(root: &Path, tree: &[String]) {
    for entry in tree {
        let path = relative(root, entry.trim_end_matches('/'));
        if entry.ends_with('/') {
            fs::create_dir_all(&path).unwrap();
        } else {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "").unwrap();
        }
    }
}

fn relative(root: &Path, path: &str) -> PathBuf {
    let mut joined = root.to_path_buf();
    joined.extend(path.split('/'));
    joined
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A fixture with only a tree, for tests that aren't in a rule file.
fn tree_only(tree: &[&str]) -> RuleTest {
    RuleTest {
        os: None,
        tree: tree.iter().map(|path| (*path).to_owned()).collect(),
        files: std::collections::BTreeMap::new(),
        git: false,
        tracked: Vec::new(),
        matches: Vec::new(),
    }
}

/// Runs one rule, alone, against a fixture, returning what it matched as `/`-separated paths
/// relative to the fixture's root.
fn run_fixture(rule: &Rule, test: &RuleTest) -> BTreeSet<String> {
    let root = tempfile::tempdir().unwrap();
    build_tree(root.path(), &test.tree);
    for (path, contents) in &test.files {
        let path = relative(root.path(), path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
    }
    if test.git {
        git(root.path(), &["init", "-q"]);
        if !test.tracked.is_empty() {
            let mut add = vec!["add", "--"];
            add.extend(test.tracked.iter().map(String::as_str));
            git(root.path(), &add);
        }
    }
    let matcher = Matcher::new([rule], &fixture_folders(root.path()), Os::current());

    let (found, report) = matcher.find(root.path()).unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let root = std::path::absolute(root.path()).unwrap();
    found
        .iter()
        .map(|detection| {
            let parts: Vec<_> = detection
                .path
                .strip_prefix(&root)
                .unwrap()
                .iter()
                .map(|part| part.to_string_lossy().into_owned())
                .collect();
            parts.join("/")
        })
        .collect()
}

fn builtin() -> RuleSet {
    RuleSet::builtin().unwrap()
}

#[test]
fn every_builtin_rule_passes_its_fixtures() {
    for rule in builtin().rules() {
        for (index, test) in rule.tests.iter().enumerate() {
            if test.os.is_some_and(|os| os != Os::current()) {
                continue;
            }
            let expected: BTreeSet<String> = test.matches.iter().cloned().collect();
            assert_eq!(
                run_fixture(rule, test),
                expected,
                "rule `{}`, test #{}",
                rule.id,
                index + 1
            );
        }
    }
}

#[test]
fn every_builtin_rule_has_positive_and_negative_fixtures() {
    for rule in builtin().rules() {
        assert!(
            rule.tests.iter().any(|test| !test.matches.is_empty()),
            "rule `{}` needs a test that matches something",
            rule.id
        );
        assert!(
            rule.tests.iter().any(|test| test.matches.is_empty()),
            "rule `{}` needs a test that matches nothing",
            rule.id
        );
    }
}

#[test]
fn every_rule_file_is_embedded() {
    let on_disk: BTreeSet<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("rules"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
    let embedded: BTreeSet<String> = BUILTIN.iter().map(|(name, _)| (*name).to_owned()).collect();

    assert_eq!(embedded, on_disk);
}

#[test]
fn records_markers_contents_and_cachedir_tags_as_evidence() {
    let rules = builtin();
    let root = tempfile::tempdir().unwrap();
    build_tree(
        root.path(),
        &[
            "proj/uv.lock".into(),
            "proj/.venv/pyvenv.cfg".into(),
            "proj/.venv/CACHEDIR.TAG".into(),
        ],
    );
    let matcher = Matcher::new(rules.rules(), &fixture_folders(root.path()), Os::current());

    let (found, _) = matcher.find(root.path()).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rule.id.0, "python.venv");
    assert_eq!(
        found[0].evidence,
        [
            Evidence::Marker {
                file: "uv.lock".into()
            },
            Evidence::Contains {
                file: "pyvenv.cfg".into()
            },
            Evidence::CacheDirTag,
        ]
    );
}

#[test]
fn records_what_git_says_for_the_safety_checks() {
    let rules = builtin();
    let root = tempfile::tempdir().unwrap();
    build_tree(
        root.path(),
        &["web/package.json".into(), "web/dist/app.js".into()],
    );
    fs::write(root.path().join(".gitignore"), "dist/\n").unwrap();
    git(root.path(), &["init", "-q"]);
    // Ignored now, but committed before it was: removing it would delete a tracked file.
    git(root.path(), &["add", "--force", "--", "web/dist/app.js"]);
    let matcher = Matcher::new(rules.rules(), &fixture_folders(root.path()), Os::current());

    let (found, _) = matcher.find(root.path()).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rule.id.0, "javascript.build_output");
    assert!(found[0].evidence.contains(&Evidence::IgnoredByGit));
    assert_eq!(
        found[0].git,
        crate::git::GitStatus::Known(crate::git::GitFacts {
            ignored: true,
            has_tracked_files: true,
        })
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn known_locations_match_regardless_of_case() {
    let rules = builtin();
    let rule = rules
        .get(&cleanse_model::RuleId("go.build_cache".into()))
        .unwrap();
    let tree = if cfg!(windows) {
        "data_local/GO-BUILD/00/x"
    } else {
        "cache/GO-BUILD/00/x"
    };

    assert_eq!(run_fixture(rule, &tree_only(&[tree])).len(), 1);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn locations_with_wildcards_match_regardless_of_case() {
    let rules = builtin();
    let rule = rules
        .get(&cleanse_model::RuleId("chrome.cache".into()))
        .unwrap();
    let tree = if cfg!(windows) {
        "data_local/GOOGLE/CHROME/USER DATA/PROFILE 3/CODE CACHE/x"
    } else {
        "cache/GOOGLE/CHROME/PROFILE 3/CODE CACHE/x"
    };

    assert_eq!(run_fixture(rule, &tree_only(&[tree])).len(), 1);
}

#[test]
fn a_wildcard_location_matches_whole_names_at_its_own_depth() {
    let rules = load(
        r#"
[[rule]]
id = "test.cache"
name = "Cache"
category = "browser_caches"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
locations = [
    { os = "windows", base = "cache", path = "app/Profile */Cache" },
    { os = "macos", base = "cache", path = "app/Profile */Cache" },
    { os = "linux", base = "cache", path = "app/Profile */Cache" },
]
"#,
    )
    .unwrap();
    let tree = tree_only(&[
        "cache/app/Profile 1/Cache/x",
        "cache/app/Profile 1/Nested/Cache/x",
        "cache/app/Profiles/Cache/x",
        "cache/app/Cache/x",
        "cache/other/Profile 1/Cache/x",
    ]);

    let found = run_fixture(&rules.rules()[0], &tree);

    assert_eq!(
        found,
        BTreeSet::from(["cache/app/Profile 1/Cache".to_owned()])
    );
}

/// A rule whose locations under `cache` are `path` on every OS, naming `entry`s.
fn location_rule(path: &str, entry: &str) -> RuleSet {
    load(&format!(
        r#"
[[rule]]
id = "test.dumps"
name = "Dumps"
category = "logs"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
entry = "{entry}"
locations = [
    {{ os = "windows", base = "cache", path = "{path}" }},
    {{ os = "macos", base = "cache", path = "{path}" }},
    {{ os = "linux", base = "cache", path = "{path}" }},
]
"#
    ))
    .unwrap()
}

#[test]
fn a_file_location_matches_only_files() {
    let rules = location_rule("app/Dumps/*.dmp", "file");
    let tree = tree_only(&[
        "cache/app/Dumps/crash.dmp",
        "cache/app/Dumps/notes.txt",
        "cache/app/Dumps/folder.dmp/x.dmp",
        "cache/app/Dumps/Nested/deeper.dmp",
        "cache/other/Dumps/crash.dmp",
    ]);

    let found = run_fixture(&rules.rules()[0], &tree);

    assert_eq!(
        found,
        BTreeSet::from(["cache/app/Dumps/crash.dmp".to_owned()])
    );
}

#[test]
fn file_locations_match_regardless_of_case() {
    let rules = location_rule("app/Dumps/*.dmp", "file");
    let tree = tree_only(&["cache/APP/DUMPS/CRASH.DMP"]);

    let found = run_fixture(&rules.rules()[0], &tree);

    assert_eq!(
        found.len(),
        usize::from(cfg!(any(windows, target_os = "macos")))
    );
}

#[test]
fn locations_match_only_the_kind_of_item_they_name() {
    let folder = location_rule("app/Cache", "dir");
    let file = location_rule("app/log.txt", "file");

    assert_eq!(
        run_fixture(&folder.rules()[0], &tree_only(&["cache/app/Cache/x"])).len(),
        1
    );
    assert!(run_fixture(&folder.rules()[0], &tree_only(&["cache/app/Cache"])).is_empty());
    assert_eq!(
        run_fixture(&file.rules()[0], &tree_only(&["cache/app/log.txt"])).len(),
        1
    );
    assert!(run_fixture(&file.rules()[0], &tree_only(&["cache/app/log.txt/x"])).is_empty());
}

#[test]
fn a_location_names_files_or_folders_but_nothing_else() {
    let rule = |entry: &str| {
        format!(
            r#"
[[rule]]
id = "test.cache"
name = "Cache"
category = "browser_caches"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
entry = "{entry}"
locations = [{{ os = "linux", base = "cache", path = "app/cache" }}]
"#
        )
    };

    assert!(load(&rule("dir")).is_ok());
    assert!(load(&rule("file")).is_ok());
    for entry in ["link", "other"] {
        let error = load(&rule(entry)).unwrap_err();
        assert!(error.contains("`dir` or `file`"), "{error}");
    }
    assert!(
        load(&rule("folder"))
            .unwrap_err()
            .contains("unknown variant")
    );
}

#[test]
fn a_locations_first_name_cant_be_a_wildcard() {
    let rule = |path: &str| {
        format!(
            r#"
[[rule]]
id = "test.cache"
name = "Cache"
category = "browser_caches"
risk = "safe"
explain = "Because."

[rule.match]
kind = "known_location"
locations = [{{ os = "linux", base = "cache", path = "{path}" }}]
"#
        )
    };

    assert!(load(&rule("app/*/cache")).is_ok());
    for path in ["*/cache", "app*/cache", "app/../*"] {
        let error = load(&rule(path)).unwrap_err();
        assert!(error.contains("only after the first name"), "{error}");
    }
}

#[test]
fn a_marker_must_be_a_real_file() {
    let rules = builtin();
    let rule = rules
        .get(&cleanse_model::RuleId("javascript.node_modules".into()))
        .unwrap();

    // A directory named like the marker isn't the marker.
    let found = run_fixture(
        rule,
        &tree_only(&["app/package.json/", "app/node_modules/x"]),
    );

    assert!(found.is_empty(), "{found:?}");
}

fn load(toml: &str) -> Result<RuleSet, String> {
    RuleSet::from_sources([("test.toml", toml)]).map_err(|error| error.to_string())
}

const VALID: &str = r#"
[[rule]]
id = "test.thing"
name = "Thing"
category = "build_output"
risk = "safe"
explain = "Because."

[rule.match]
kind = "project_artifact"
names = ["thing"]
markers = ["thing.toml"]
"#;

#[test]
fn loads_a_valid_rule() {
    let rules = load(VALID).unwrap();

    assert_eq!(rules.rules().len(), 1);
}

#[test]
fn rejects_rules_that_could_escape_or_misbehave() {
    let cases = [
        // Unknown fields are almost always typos, like `marker` for `markers`.
        (VALID.replace("markers", "marker"), "unknown field"),
        (
            VALID.replace(r#"markers = ["thing.toml"]"#, ""),
            "a directory name alone isn't evidence",
        ),
        (
            VALID.replace("thing.toml", "../thing.toml"),
            "relative path",
        ),
        (VALID.replace("thing.toml", "/etc/passwd"), "relative path"),
        (
            VALID.replace("thing.toml", "C:/thing.toml"),
            "relative path",
        ),
        (
            VALID.replace("thing.toml", r"sub\\thing.toml"),
            "relative path",
        ),
        (VALID.replace("thing.toml", "file:stream"), "relative path"),
        (
            VALID.replace(r#"names = ["thing"]"#, r#"names = ["a/b"]"#),
            "single file or directory name",
        ),
        (VALID.replace("test.thing", "Test.Thing"), "valid id"),
        (VALID.replace("test.thing", "thing"), "valid id"),
        (
            VALID.replace(r#"risk = "safe""#, r#"risk = "yolo""#),
            "unknown variant",
        ),
        (VALID.replace("Because.", " "), "`explain` is empty"),
        (format!("{VALID}{VALID}"), "already uses this id"),
        (
            format!("{VALID}\n[rule.owner]\napp = \"Thing\"\nprocesses = []\n"),
            "`owner.processes` is empty",
        ),
        (
            format!("{VALID}\n[rule.owner]\napp = \"Thing\"\nprocesses = [\"Thing.exe\"]\n"),
            "lowercase program name",
        ),
    ];
    for (toml, expected) in cases {
        let error = load(&toml).unwrap_err();
        assert!(
            error.contains(expected),
            "expected {expected:?} in {error:?}"
        );
    }
}

#[test]
fn rejects_two_rules_claiming_the_same_location() {
    let rule = |id: &str| {
        format!(
            r#"
[[rule]]
id = "{id}"
name = "Cache"
category = "package_caches"
risk = "regenerable"
explain = "Because."

[rule.match]
kind = "known_location"
locations = [{{ os = "linux", base = "cache", path = "tool" }}]
"#
        )
    };

    let error = load(&format!("{}{}", rule("one.cache"), rule("two.cache"))).unwrap_err();

    assert!(error.contains("already claims"), "{error}");
}

#[test]
fn wildcards_match_any_run_of_characters() {
    assert!(wildcard_match("*.csproj", "App.csproj"));
    assert!(wildcard_match("cmake-build-*", "cmake-build-debug"));
    assert!(wildcard_match("a*b*c", "a-x-b-y-c"));
    assert!(wildcard_match("exact", "exact"));
    assert!(!wildcard_match("*.csproj", "App.csproj.bak"));
    assert!(!wildcard_match("a*a", "a"));
    assert!(!wildcard_match("exact", "exactly"));
}

#[test]
fn rules_can_be_turned_off_by_id_but_not_by_typo() {
    let node_modules = RuleId("javascript.node_modules".into());

    let without = RuleSet::builtin()
        .unwrap()
        .without(std::slice::from_ref(&node_modules));
    let typo = RuleSet::builtin()
        .unwrap()
        .without(&[RuleId("javascript.node_module".into())]);

    let without = without.unwrap();
    assert!(without.get(&node_modules).is_none());
    assert_eq!(
        without.rules().len(),
        RuleSet::builtin().unwrap().rules().len() - 1
    );
    assert!(typo.unwrap_err().contains("javascript.node_module"));
}
