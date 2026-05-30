//! Purpose: Classify requested shell commands before the proxy chooses a reducer.
//! Caller: proxy::run and command adapters.
//! Dependencies: std::path for preserving the caller working directory.
//! Main Functions: CommandAst::new, CommandAst::classify.
//! Side Effects: None; classification is in-memory only.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum CommandKind {
    Test,
    Git,
    Search,
    FileRead,
    FileList,
    Build,
    Lint,
    Logs,
    Container,
    Cloud,
    Database,
    PackageManager,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CommandAst {
    pub original_command: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub shell_wrapped: bool,
    pub detected_kind: CommandKind,
    pub has_shell_syntax: bool,
}

impl CommandAst {
    pub fn new(program: String, args: Vec<String>, cwd: PathBuf) -> Self {
        let original_command = format!("{} {}", program, args.join(" "));
        Self::from_parts(original_command, program, args, cwd, false, false)
    }

    pub fn from_command_text(command: &str, cwd: PathBuf) -> Self {
        let mut fields = command.split_whitespace();
        let program = fields.next().unwrap_or(command).to_string();
        let args = fields.map(str::to_string).collect();
        Self::from_parts(command.to_string(), program, args, cwd, false, false)
    }

    pub fn from_parts(
        original_command: String,
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        shell_wrapped: bool,
        has_shell_syntax: bool,
    ) -> Self {
        let mut ast = Self {
            original_command,
            program,
            args,
            cwd,
            shell_wrapped,
            detected_kind: CommandKind::Unknown,
            has_shell_syntax,
        };
        ast.classify();
        ast
    }

    pub fn classify(&mut self) {
        let program_base = self.program_base_name();
        self.detected_kind = match program_base.as_str() {
            "cargo" => {
                if self
                    .args
                    .iter()
                    .any(|arg| arg == "test" || arg == "nextest")
                {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| arg == "clippy") {
                    CommandKind::Lint
                } else if self.args.iter().any(|arg| arg == "build" || arg == "check") {
                    CommandKind::Build
                } else {
                    CommandKind::Unknown
                }
            }
            "pytest" | "jest" | "vitest" | "playwright" => CommandKind::Test,
            // Additional test runners across ecosystems (route to the tests
            // adapter: pass/fail summary + failure signal + rerun hint).
            "mocha" | "ava" | "cypress" | "karma" | "tap" | "gotestsum" | "bats" | "ctest" => {
                CommandKind::Test
            }
            // Python meta test runners. `tox`/`nox` orchestrate test envs; their
            // output is test output.
            "tox" | "nox" => CommandKind::Test,
            "deno" => {
                if self.args.iter().any(|arg| arg == "test") {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| arg == "lint") {
                    CommandKind::Lint
                } else if self.args.iter().any(|arg| arg == "fmt") {
                    CommandKind::Build
                } else {
                    CommandKind::Unknown
                }
            }
            "python" => {
                if self.args.first().map(String::as_str) == Some("-m")
                    && self.args.get(1).map(String::as_str) == Some("pytest")
                {
                    CommandKind::Test
                } else {
                    CommandKind::Unknown
                }
            }
            "npx" | "pnpx" | "dlx" => {
                if self
                    .args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "jest" | "vitest" | "playwright"))
                {
                    CommandKind::Test
                } else {
                    CommandKind::PackageManager
                }
            }
            "go" => {
                if self.args.iter().any(|arg| arg == "test") {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "build" | "vet" | "mod" | "get" | "install" | "fmt"
                    )
                }) {
                    CommandKind::Build
                } else {
                    CommandKind::Unknown
                }
            }
            "mvn" | "gradle" | "dotnet" => {
                if self.args.iter().any(|arg| arg == "test") {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| arg == "build" || arg == "vet") {
                    CommandKind::Build
                } else {
                    CommandKind::Unknown
                }
            }
            "npm" | "pnpm" | "yarn" | "bun" => {
                if self.args.iter().any(|arg| {
                    matches!(arg.as_str(), "test" | "jest" | "vitest") || arg.ends_with(":test")
                }) {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| arg == "build") {
                    CommandKind::Build
                } else if self.args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "install"
                            | "i"
                            | "ci"
                            | "add"
                            | "update"
                            | "up"
                            | "remove"
                            | "rm"
                            | "ls"
                            | "list"
                            | "outdated"
                            | "audit"
                    )
                }) {
                    CommandKind::PackageManager
                } else {
                    CommandKind::Unknown
                }
            }
            "git" if self.args.first().map(String::as_str) == Some("grep") => CommandKind::Search,
            "git" | "gh" | "git-lfs" => CommandKind::Git,
            // Jujutsu (jj) is a Git-compatible VCS; its status/log/diff output is
            // the same shape as git, so route it to the git adapter.
            "jj" => CommandKind::Git,
            "rg" | "grep" => CommandKind::Search,
            "cat" | "head" | "tail" | "sed" | "jq" | "awk" | "sort" | "uniq" | "wc" | "cut"
            | "tr" | "tee" => CommandKind::FileRead,
            "diff" | "wdiff" | "colordiff" | "comm" | "cmp" => CommandKind::FileRead,
            "file" | "stat" | "echo" | "printf" | "env" | "printenv" => CommandKind::FileRead,
            "xxd" | "hexdump" | "od" => CommandKind::FileRead,
            "ls" | "find" | "tree" | "dir" => CommandKind::FileList,
            "which" | "whereis" | "where" | "type" | "whatis" => CommandKind::FileList,
            "tsc" | "prettier" | "esbuild" | "swc" | "rollup" | "parcel" | "turbo" => {
                CommandKind::Build
            }
            "make" | "cmake" | "ninja" | "xcodebuild" | "msbuild" => CommandKind::Build,
            // Additional build systems across ecosystems (route to the build
            // adapter: errors first, repeated noise removed).
            "bazel" | "buck" | "buck2" | "meson" | "scons" | "bear" => CommandKind::Build,
            // C/C++ compilers and task runners — high-volume, error-first output.
            "gcc" | "g++" | "clang" | "clang++" | "cc" | "c++" => CommandKind::Build,
            "just" | "task" | "mise" | "trunk" | "pre-commit" => CommandKind::Build,
            // Swift / OpenTofu / Ansible / DB-migration / doc build tooling.
            "swift" | "tofu" | "ansible" | "ansible-playbook" => CommandKind::Build,
            "liquibase" | "flyway" | "quarto" => CommandKind::Build,
            // JVM / other-language build+test multiplexers. Test/build split is
            // by subcommand so a `test` task still routes to the tests adapter.
            "sbt" | "lein" | "mill" => {
                if self.args.iter().any(|arg| arg.contains("test")) {
                    CommandKind::Test
                } else {
                    CommandKind::Build
                }
            }
            // Haskell (cabal/stack) and Elixir (mix) build+test multiplexers.
            "cabal" | "stack" | "mix" => {
                if self
                    .args
                    .iter()
                    .any(|arg| arg == "test" || arg.ends_with(".test"))
                {
                    CommandKind::Test
                } else {
                    CommandKind::Build
                }
            }
            "chmod" | "chown" | "touch" | "mkdir" | "rm" | "cp" | "mv" | "ln" | "rmdir" => {
                CommandKind::Build
            }
            "dd" | "install" | "strip" => CommandKind::Build,
            "eslint" | "ruff" | "mypy" | "biome" => CommandKind::Lint,
            // Additional linters/formatters-as-checkers across ecosystems
            // (route to the lint adapter: error+warning signal first).
            "pylint" | "pyright" | "shellcheck" | "hadolint" | "yamllint" | "stylelint"
            | "tflint" | "clippy-driver" | "oxlint" | "standard" | "luacheck" | "vale" => {
                CommandKind::Lint
            }
            // More checkers across ecosystems: typed-Python (basedpyright/ty),
            // markdown, and the trunk meta-linter route to the lint adapter.
            "basedpyright" | "ty" | "markdownlint" | "markdownlint-cli2" => CommandKind::Lint,
            "docker" | "kubectl" | "helm" | "podman" | "nerdctl" => CommandKind::Container,
            "aws" | "az" | "gcloud" => CommandKind::Cloud,
            "terraform" => CommandKind::Logs,
            "curl" | "wget" | "http" | "xh" | "httpie" => CommandKind::Logs,
            "journalctl" | "systemctl" | "loginctl" => CommandKind::Logs,
            "pip" | "pip3" => {
                if self.args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "list" | "freeze" | "show" | "install" | "uninstall" | "download"
                    )
                }) {
                    CommandKind::PackageManager
                } else {
                    CommandKind::Build
                }
            }
            "rake" => {
                if self.args.iter().any(|arg| arg == "test") {
                    CommandKind::Test
                } else {
                    CommandKind::Build
                }
            }
            "rspec" | "phpunit" => CommandKind::Test,
            "rubocop" | "flake8" | "golangci-lint" => CommandKind::Lint,
            "black" | "isort" => CommandKind::Build,
            "bundle" | "composer" => {
                if self
                    .args
                    .iter()
                    .any(|arg| ["install", "update", "add", "require"].contains(&arg.as_str()))
                {
                    CommandKind::PackageManager
                } else {
                    CommandKind::Build
                }
            }
            "webpack" | "vite" | "next" => CommandKind::Build,
            "nx" => {
                if self.args.iter().any(|arg| arg == "test") {
                    CommandKind::Test
                } else if self.args.iter().any(|arg| arg == "lint") {
                    CommandKind::Lint
                } else if self.args.iter().any(|arg| arg == "build") {
                    CommandKind::Build
                } else {
                    CommandKind::Unknown
                }
            }
            "brew" | "apt" | "apt-get" | "apk" | "choco" | "winget" | "scoop" => {
                CommandKind::PackageManager
            }
            // uv is the fast Python package manager; install/sync/lock are
            // package-manager operations, `uv run`/`uv build` are build-like.
            "uv" => {
                if self.args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "pip" | "add" | "remove" | "sync" | "lock" | "install" | "tree"
                    )
                }) {
                    CommandKind::PackageManager
                } else {
                    CommandKind::Build
                }
            }
            "df" | "du" | "ncdu" => CommandKind::FileList,
            "ps" | "top" | "htop" | "pgrep" | "pidof" | "kill" | "killall" | "pkill" | "pwdx"
            | "lsof" | "fuser" => CommandKind::Container,
            "netstat" | "ss" | "ping" | "traceroute" | "nslookup" | "dig" | "host" | "nc"
            | "telnet" | "nmap" | "tcpdump" | "iperf" | "iperf3" => CommandKind::Logs,
            "ip" | "ifconfig" | "route" | "arp" | "brctl" | "iptables" | "ufw" => CommandKind::Logs,
            "tar" | "zip" | "unzip" | "gzip" | "gunzip" | "bzip2" | "xz" | "7z" | "rar"
            | "unrar" => CommandKind::Build,
            "scp" | "rsync" | "sftp" | "ssh" | "ssh-keygen" | "ssh-keyscan" | "ssh-copy-id"
            | "socat" => CommandKind::Logs,
            "openssl" | "gpg" | "gpg2" | "age" | "age-keygen" => CommandKind::Logs,
            "date" | "uptime" | "uname" | "hostname" | "whoami" | "who" | "id" | "groups"
            | "locale" => CommandKind::FileRead,
            "psql" | "sqlite3" | "mysql" | "mariadb" | "redis-cli" | "mongosh" | "mongo" => {
                CommandKind::Database
            }
            "pg_dump" | "pg_dumpall" | "pg_restore" | "mysqldump" => CommandKind::Logs,
            _ => CommandKind::Unknown,
        };
    }

    fn program_base_name(&self) -> String {
        let normalized = self.program.replace('\\', "/");
        let base_name = normalized.rsplit('/').next().unwrap_or(&self.program);
        base_name
            .trim_end_matches(".exe")
            .trim_end_matches(".cmd")
            .trim_end_matches(".bat")
            .to_ascii_lowercase()
    }
}
