//! Tests against Runnel itself: a temporary project with the plugin, trusted.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The environment variable with the path of the runnel program for tests against Runnel
/// itself ([`Project::new`]); without it, those tests are skipped.
pub const RUNNEL_ENV: &str = "RUNNEL_BIN";

/// A temporary Runnel project with the plugin under test, installed and trusted. The
/// folder is removed when the project is dropped.
pub struct Project {
    runnel: PathBuf,
    root: PathBuf,
    /// The project folder.
    pub dir: PathBuf,
}

impl Project {
    /// Creates a project in a temporary folder for tests against Runnel: it copies the
    /// program of the plugin to `plugins/<name>/bin/<name>[.exe]` and `plugin_yaml` (the
    /// plugin.yaml of the plugin, whose executable must point there) next to it, and
    /// trusts the plugin with `runnel plugins trust --yes`. The trust file lies in the
    /// temporary folder too, so the test does not touch the trusts of the user.
    ///
    /// Returns None, and the test should return early, unless `RUNNEL_BIN` names the
    /// runnel program. In an integration test of a plugin crate, the program is
    /// `env!("CARGO_BIN_EXE_<name>")`.
    pub fn new(plugin_yaml: impl AsRef<Path>, executable: impl AsRef<Path>) -> Option<Project> {
        let Some(runnel) = std::env::var_os(RUNNEL_ENV).filter(|v| !v.is_empty()) else {
            eprintln!("{RUNNEL_ENV} is not set; set it to the runnel program to test against Runnel (test skipped)");
            return None;
        };
        let yaml = std::fs::read_to_string(plugin_yaml.as_ref()).expect("read plugin.yaml");
        let name = yaml
            .lines()
            .find_map(|l| l.strip_prefix("name:"))
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .expect("plugin.yaml without name");
        let root = std::env::temp_dir().join(format!("runnel-plugin-test-{}-{}", std::process::id(), unique()));
        let p = Project {
            runnel: PathBuf::from(runnel),
            dir: root.join("project"),
            root,
        };
        let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let bin = p.dir.join("plugins").join(&name).join("bin");
        std::fs::create_dir_all(&bin).expect("create plugin folder");
        std::fs::copy(executable.as_ref(), bin.join(&exe)).expect("copy plugin program");
        p.write_file(&format!("plugins/{name}/plugin.yaml"), &yaml);
        p.write_file(
            "project.yaml",
            "kind: Project\napiVersion: runnel/v1\nname: plugin-test\n",
        );
        let dir = p.dir.to_string_lossy().to_string();
        let (out, code) = p.runnel(&["plugins", "trust", &dir, "--yes"]);
        assert_eq!(code, 0, "runnel plugins trust: exit {code}\n{out}");
        Some(p)
    }

    /// Writes a file of the project, e.g. `"workflows/check.workflow.yaml"`.
    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create folder");
        std::fs::write(&path, content).expect("write file");
    }

    /// Runs the runnel program with args and returns its output (stdout and stderr) and
    /// exit code.
    pub fn runnel(&self, args: &[&str]) -> (String, i32) {
        let config = self.root.join("config");
        let out = Command::new(&self.runnel)
            .args(args)
            .current_dir(&self.dir)
            .env("XDG_CONFIG_HOME", &config) // trusts on Linux
            .env("APPDATA", &config) // trusts on Windows
            .env("HOME", &config) // trusts on macOS
            .env("LOCALAPPDATA", self.root.join("local"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .output()
            .unwrap_or_else(|e| panic!("runnel {args:?}: {e}"));
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (text, out.status.code().unwrap_or(-1))
    }

    /// Runs a workflow or pipeline of the project (path relative to the project) with
    /// `runnel run` and parameters `KEY=VALUE`, without run history, and returns its log
    /// and exit code.
    pub fn run(&self, file: &str, params: &[&str]) -> (String, i32) {
        let path = self
            .dir
            .join(file.replace('/', std::path::MAIN_SEPARATOR_STR))
            .to_string_lossy()
            .to_string();
        let mut args = vec!["run", "--history", "none", path.as_str()];
        for kv in params {
            args.push("--param");
            args.push(kv);
        }
        self.runnel(&args)
    }

    /// Runs `runnel validate` on the project and returns its output and exit code.
    pub fn validate(&self) -> (String, i32) {
        let dir = self.dir.to_string_lossy().to_string();
        self.runnel(&["validate", &dir])
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn unique() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    t + u128::from(N.fetch_add(1, Ordering::Relaxed))
}
