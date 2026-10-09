//! Per-language tools: the language server and the formatter for a file.
//!
//! Defaults cover common languages whose tools are on `PATH`.
//! `languages.toml` in the config directory overrides or adds entries:
//!
//! ```toml
//! [[language]]
//! name = "Python"                  # syntax name shown in the status bar
//! extensions = ["py", "pyi"]       # optional: match by extension instead
//! language-id = "python"           # LSP languageId (optional)
//! server = ["pyright-langserver", "--stdio"]
//! formatter = ["black", "-q", "-"]  # reads stdin, writes stdout
//! roots = ["pyproject.toml"]        # markers of a project root
//! ```
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Language {
    pub name: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub language_id: Option<String>,
    /// Candidate servers; the first one installed is used.
    #[serde(default, deserialize_with = "one_or_many")]
    pub server: Vec<Vec<String>>,
    #[serde(default)]
    pub formatter: Option<Vec<String>>,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub initialization_options: serde_json::Value,
}

/// `server = ["a", "--x"]` or `server = [["a"], ["b"]]`.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Vec<String>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Form {
        One(Vec<String>),
        Many(Vec<Vec<String>>),
    }
    Ok(match Form::deserialize(d)? {
        Form::One(v) if v.is_empty() => vec![],
        Form::One(v) => vec![v],
        Form::Many(v) => v,
    })
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    language: Vec<Language>,
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn builtin() -> Vec<Language> {
    let lang =
        |name: &str, id: &str, servers: &[&[&str]], formatter: Option<&[&str]>, roots: &[&str]| {
            Language {
                name: name.into(),
                extensions: vec![],
                language_id: Some(id.into()),
                server: servers.iter().map(|s| strings(s)).collect(),
                formatter: formatter.map(strings),
                roots: strings(roots),
                settings: serde_json::Value::Null,
                initialization_options: serde_json::Value::Null,
            }
        };
    vec![
        lang(
            "Rust",
            "rust",
            &[&["rust-analyzer"]],
            Some(&["rustfmt", "--edition", "2021", "--emit", "stdout"]),
            &["Cargo.toml"],
        ),
        lang(
            "Python",
            "python",
            &[
                &["pyright-langserver", "--stdio"],
                &["basedpyright-langserver", "--stdio"],
                &["pylsp"],
                &["jedi-language-server"],
            ],
            Some(&["black", "-q", "-"]),
            &["pyproject.toml", "setup.py", "setup.cfg"],
        ),
        lang(
            "JavaScript",
            "javascript",
            &[&["typescript-language-server", "--stdio"]],
            Some(&["prettier", "--stdin-filepath", "${file}"]),
            &["package.json", "jsconfig.json"],
        ),
        lang(
            "TypeScript",
            "typescript",
            &[&["typescript-language-server", "--stdio"]],
            Some(&["prettier", "--stdin-filepath", "${file}"]),
            &["package.json", "tsconfig.json"],
        ),
        lang(
            "TypeScriptReact",
            "typescriptreact",
            &[&["typescript-language-server", "--stdio"]],
            Some(&["prettier", "--stdin-filepath", "${file}"]),
            &["package.json", "tsconfig.json"],
        ),
        lang(
            "C",
            "c",
            &[&["clangd"]],
            Some(&["clang-format", "--assume-filename=${file}"]),
            &["compile_commands.json", "CMakeLists.txt", "Makefile"],
        ),
        lang(
            "C++",
            "cpp",
            &[&["clangd"]],
            Some(&["clang-format", "--assume-filename=${file}"]),
            &["compile_commands.json", "CMakeLists.txt", "Makefile"],
        ),
        lang("Go", "go", &[&["gopls"]], Some(&["gofmt"]), &["go.mod"]),
        lang(
            "Bourne Again Shell (bash)",
            "shellscript",
            &[&["bash-language-server", "start"]],
            Some(&["shfmt", "-"]),
            &[],
        ),
        lang(
            "Lua",
            "lua",
            &[&["lua-language-server"]],
            Some(&["stylua", "-"]),
            &[],
        ),
        lang(
            "JSON",
            "json",
            &[&["vscode-json-language-server", "--stdio"]],
            None,
            &[],
        ),
        lang(
            "YAML",
            "yaml",
            &[&["yaml-language-server", "--stdio"]],
            None,
            &[],
        ),
        lang(
            "Markdown",
            "markdown",
            &[&["marksman", "server"]],
            None,
            &[],
        ),
        lang(
            "Zig",
            "zig",
            &[&["zls"]],
            Some(&["zig", "fmt", "--stdin"]),
            &["build.zig"],
        ),
        lang(
            "Java",
            "java",
            &[&["jdtls"]],
            None,
            &["pom.xml", "build.gradle"],
        ),
        lang(
            "Ruby",
            "ruby",
            &[&["solargraph", "stdio"]],
            None,
            &["Gemfile"],
        ),
        lang(
            "HTML",
            "html",
            &[&["vscode-html-language-server", "--stdio"]],
            None,
            &[],
        ),
        lang(
            "CSS",
            "css",
            &[&["vscode-css-language-server", "--stdio"]],
            None,
            &[],
        ),
        lang(
            "TOML",
            "toml",
            &[&["taplo", "lsp", "stdio"]],
            Some(&["taplo", "fmt", "-"]),
            &[],
        ),
        lang("Nix", "nix", &[&["nil"]], Some(&["nixfmt"]), &["flake.nix"]),
    ]
}

/// User languages first (they override built-in ones of the same name).
/// A `languages.toml` that does not parse is added to `errors`.
pub fn all(errors: &mut Vec<String>) -> Vec<Language> {
    let path = crate::paths::config_dir().join("languages.toml");
    let mut list = crate::config::list(&path, |f: File| f.language, errors);
    for language in builtin() {
        if !list.iter().any(|l| l.name == language.name) {
            list.push(language);
        }
    }
    list
}

/// The language of a file: by configured extension, else by syntax name.
pub fn find<'a>(languages: &'a [Language], path: &Path, syntax: &str) -> Option<&'a Language> {
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    languages
        .iter()
        .find(|l| extension.as_ref().is_some_and(|e| l.extensions.contains(e)))
        .or_else(|| languages.iter().find(|l| l.name == syntax))
}

impl Language {
    pub fn id(&self) -> String {
        self.language_id
            .clone()
            .unwrap_or_else(|| self.name.to_lowercase())
    }
    /// The first configured server whose program is installed.
    pub fn server_command(&self) -> Option<Vec<String>> {
        self.server
            .iter()
            .find(|c| {
                c.first()
                    .is_some_and(|p| crate::fsio::which(p).is_some() || Path::new(p).is_file())
            })
            .cloned()
    }
    /// The project root for a file: the workspace for files inside it;
    /// otherwise the nearest folder with a root marker, else the file's
    /// folder.
    pub fn root_for(&self, file: &Path, workspace: &Path) -> PathBuf {
        if file.starts_with(workspace) {
            return workspace.to_path_buf();
        }
        let mut dir = file.parent();
        while let Some(d) = dir {
            if self.roots.iter().any(|m| d.join(m).exists()) {
                return d.to_path_buf();
            }
            dir = d.parent();
        }
        file.parent().unwrap_or(workspace).to_path_buf()
    }
}

/// Expand `${file}` in a command.
pub fn expand(command: &[String], file: &Path) -> Vec<String> {
    command
        .iter()
        .map(|part| part.replace("${file}", &file.to_string_lossy()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_entries_override_and_extensions_match() {
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        std::fs::create_dir_all(dir.path().join("slate")).unwrap();
        std::fs::write(
            dir.path().join("slate/languages.toml"),
            "[[language]]\nname = \"Rust\"\nserver = [\"my-ra\"]\n\n[[language]]\nname = \"Fake\"\nextensions = [\"fk\"]\nserver = [[\"a\"], [\"b\", \"-x\"]]\n",
        )
        .unwrap();
        let mut errors = Vec::new();
        let languages = all(&mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        let for_file =
            |path: &str, syntax: &str| find(&languages, Path::new(path), syntax).cloned();
        let rust = for_file("/x/a.rs", "Rust").unwrap();
        assert_eq!(rust.server, vec![vec!["my-ra".to_string()]]);
        assert_eq!(rust.id(), "rust");
        let fake = for_file("/x/a.fk", "Plain Text").unwrap();
        assert_eq!(fake.server.len(), 2);
        assert_eq!(fake.id(), "fake");
        assert!(for_file("/x/a.txt", "Plain Text").is_none());
        let go = for_file("/x/a.go", "Go").unwrap();
        assert_eq!(go.formatter, Some(vec!["gofmt".to_string()]));
        assert_eq!(
            expand(
                &strings(&["prettier", "--stdin-filepath", "${file}"]),
                Path::new("/a b.ts")
            ),
            ["prettier", "--stdin-filepath", "/a b.ts"]
        );
        // A file that does not parse is reported; built-in languages remain.
        std::fs::write(
            dir.path().join("slate/languages.toml"),
            "[[language]]\nname = 3\n",
        )
        .unwrap();
        let mut errors = Vec::new();
        assert!(all(&mut errors).iter().any(|l| l.name == "Rust"));
        assert!(errors[0].contains("languages.toml"), "{errors:?}");
    }
}
