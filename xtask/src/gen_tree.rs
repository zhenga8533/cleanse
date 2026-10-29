//! Builds a deterministic synthetic directory tree for walker benchmarks.
//!
//! The shape imitates a folder of JavaScript and Rust projects: wide `node_modules` with nested
//! packages, build output, source files, a few large files, a deep chain, and one path longer
//! than Windows' `MAX_PATH`. Most files are small, so the tree stays a few hundred megabytes.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DEFAULT_ENTRIES: u64 = 200_000;
static FILL: [u8; 32 * 1024] = [b'x'; 32 * 1024];

pub fn run(args: &[String]) -> ExitCode {
    let (dir, entries) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}\n\nusage: cargo xtask gen-tree <dir> [--entries <n>]");
            return ExitCode::from(2);
        }
    };
    if fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_some()) {
        eprintln!("refusing to write into {}: it isn't empty", dir.display());
        return ExitCode::FAILURE;
    }

    let mut tree = Generator {
        remaining: entries,
        bytes: 0,
        rng: Rng(0x2545_F491_4F6C_DD1D),
    };
    match tree.generate(&dir) {
        Ok(()) => {
            eprintln!(
                "created {entries} entries ({} MiB of file data) in {}",
                tree.bytes / (1024 * 1024),
                dir.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to generate the tree: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<(PathBuf, u64), String> {
    let mut dir = None;
    let mut entries = DEFAULT_ENTRIES;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--entries" {
            let value = args.next().ok_or("--entries needs a value")?;
            entries = value
                .parse()
                .map_err(|_| format!("invalid entry count `{value}`"))?;
        } else if dir.is_none() {
            dir = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument `{arg}`"));
        }
    }
    Ok((dir.ok_or("missing <dir>")?, entries))
}

/// A tiny xorshift generator, so the tree is identical on every run and machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.next() % (high - low)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

struct Generator {
    remaining: u64,
    bytes: u64,
    rng: Rng,
}

impl Generator {
    fn generate(&mut self, root: &Path) -> io::Result<()> {
        fs::create_dir_all(root)?;
        self.special_cases(root)?;
        let mut project = 0;
        while self.remaining > 0 {
            self.project(&root.join(format!("project-{project:04}")))?;
            project += 1;
        }
        Ok(())
    }

    /// Creates a directory, returning `false` once the entry budget is spent.
    fn dir(&mut self, path: &Path) -> io::Result<bool> {
        if self.remaining == 0 {
            return Ok(false);
        }
        fs::create_dir(path)?;
        self.remaining -= 1;
        Ok(true)
    }

    /// Creates a file of `len` bytes, returning `false` once the entry budget is spent.
    fn file(&mut self, path: &Path, len: u64) -> io::Result<bool> {
        if self.remaining == 0 {
            return Ok(false);
        }
        if let Some(fill) = usize::try_from(len).ok().and_then(|len| FILL.get(..len)) {
            fs::write(path, fill)?;
        } else {
            File::create(path)?.set_len(len)?;
        }
        self.remaining -= 1;
        self.bytes += len;
        Ok(true)
    }

    fn special_cases(&mut self, root: &Path) -> io::Result<()> {
        let large = root.join("large");
        if self.dir(&large)? {
            for i in 0..5 {
                self.file(&large.join(format!("video-{i}.mp4")), 2 * 1024 * 1024)?;
            }
        }

        let mut deep = root.join("deep");
        for level in 0..40 {
            if !self.dir(&deep)? {
                return Ok(());
            }
            deep.push(format!("d{level:02}"));
        }
        self.file(&deep.with_extension("txt"), 10)?;

        let mut long = root.join("long");
        for _ in 0..12 {
            if !self.dir(&long)? {
                return Ok(());
            }
            long.push("a-directory-name-thirty-chars-");
        }
        self.file(&long.with_extension("txt"), 10)?;
        Ok(())
    }

    fn project(&mut self, dir: &Path) -> io::Result<()> {
        if !self.dir(dir)? {
            return Ok(());
        }
        self.file(&dir.join("package.json"), 600)?;
        self.file(&dir.join("README.md"), 1500)?;

        let src = dir.join("src");
        if self.dir(&src)? {
            for i in 0..self.rng.range(10, 30) {
                let len = self.rng.range(200, 3000);
                self.file(&src.join(format!("module_{i}.ts")), len)?;
            }
        }

        let node_modules = dir.join("node_modules");
        if self.dir(&node_modules)? {
            for i in 0..self.rng.range(20, 80) {
                self.package(&node_modules.join(format!("package-{i}")), 0)?;
            }
        }

        let deps = dir.join("target");
        if self.dir(&deps)? {
            let deps = deps.join("deps");
            if self.dir(&deps)? {
                for i in 0..self.rng.range(10, 30) {
                    let len = self.rng.range(4096, 32 * 1024);
                    self.file(&deps.join(format!("libcrate_{i}.rlib")), len)?;
                }
            }
        }
        Ok(())
    }

    fn package(&mut self, dir: &Path, depth: u32) -> io::Result<()> {
        if !self.dir(dir)? {
            return Ok(());
        }
        let len = self.rng.range(300, 700);
        self.file(&dir.join("package.json"), len)?;
        let len = self.rng.range(100, 1500);
        self.file(&dir.join("index.js"), len)?;

        let lib = dir.join("lib");
        if self.dir(&lib)? {
            for i in 0..self.rng.range(3, 15) {
                let len = self.rng.range(50, 1200);
                self.file(&lib.join(format!("file_{i}.js")), len)?;
            }
        }

        if depth < 3 && self.rng.chance(30) {
            let nested = dir.join("node_modules");
            if self.dir(&nested)? {
                for i in 0..self.rng.range(1, 5) {
                    self.package(&nested.join(format!("package-{i}")), depth + 1)?;
                }
            }
        }
        Ok(())
    }
}
