//! Which language a file is written in (from its name), the grammar that colors it and the
//! formatter that tidies it.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Java,
    Kotlin,
    JavaScript,
    /// JavaScript with JSX.
    Jsx,
    TypeScript,
    Tsx,
    Json,
    Yaml,
    Toml,
    Markdown,
    Rust,
    Python,
    Go,
    Shell,
    Html,
    Css,
    Scss,
    Less,
    Sql,
    Xml,
    Dockerfile,
    /// Anything else: colored when a bundled grammar knows the extension, otherwise plain.
    Other,
}

/// Formatters Agentty knows how to call (only ever the installed ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formatter {
    Prettier,
    GoogleJavaFormat,
    Ktlint,
    Rustfmt,
    Gofmt,
    Black,
}

impl Language {
    pub fn detect(path: &Path) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        match name.as_str() {
            "dockerfile" | "containerfile" => return Self::Dockerfile,
            ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile" | ".envrc" => return Self::Shell,
            "cargo.lock" | "pipfile" | "poetry.lock" => return Self::Toml,
            _ if name.starts_with("dockerfile.") => return Self::Dockerfile,
            _ => {}
        }
        let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        match extension.as_str() {
            "java" => Self::Java,
            "kt" | "kts" => Self::Kotlin,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "jsx" => Self::Jsx,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "json" | "jsonc" | "json5" | "webmanifest" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "md" | "markdown" | "mdx" => Self::Markdown,
            "rs" => Self::Rust,
            "py" | "pyi" | "pyw" => Self::Python,
            "go" => Self::Go,
            "sh" | "bash" | "zsh" | "ksh" | "command" => Self::Shell,
            "html" | "htm" | "xhtml" => Self::Html,
            "css" => Self::Css,
            "scss" => Self::Scss,
            "less" => Self::Less,
            "sql" => Self::Sql,
            "xml" | "xsd" | "svg" | "plist" | "pom" => Self::Xml,
            _ => Self::Other,
        }
    }

    /// Name shown in the editor's status line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Java => "Java",
            Self::Kotlin => "Kotlin",
            Self::JavaScript => "JavaScript",
            Self::Jsx => "JavaScript React",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TypeScript React",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Markdown => "Markdown",
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Go => "Go",
            Self::Shell => "Shell",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Scss => "SCSS",
            Self::Less => "Less",
            Self::Sql => "SQL",
            Self::Xml => "XML",
            Self::Dockerfile => "Dockerfile",
            Self::Other => "",
        }
    }

    /// Names of the bundled grammar (bat's set, see `highlight.rs`), first match wins.
    pub fn grammars(self) -> &'static [&'static str] {
        match self {
            Self::Java => &["Java"],
            Self::Kotlin => &["Kotlin"],
            Self::JavaScript => &["JavaScript (Babel)", "JavaScript"],
            // The TSX grammar reads JSX too (Babel's is left out of the pure-Rust regex set).
            Self::Jsx => &["JavaScript (Babel)", "TypeScriptReact", "JavaScript"],
            Self::TypeScript => &["TypeScript"],
            Self::Tsx => &["TypeScriptReact", "TypeScript"],
            Self::Json => &["JSON"],
            Self::Yaml => &["YAML"],
            Self::Toml => &["TOML"],
            Self::Markdown => &["Markdown"],
            Self::Rust => &["Rust"],
            Self::Python => &["Python"],
            Self::Go => &["Go"],
            Self::Shell => &["Bourne Again Shell (bash)"],
            Self::Html => &["HTML"],
            Self::Css => &["CSS"],
            Self::Scss => &["SCSS", "CSS"],
            Self::Less => &["LESS", "CSS"],
            Self::Sql => &["SQL"],
            Self::Xml => &["XML"],
            Self::Dockerfile => &["Dockerfile"],
            Self::Other => &[],
        }
    }

    pub fn formatter(self) -> Option<Formatter> {
        match self {
            Self::JavaScript
            | Self::Jsx
            | Self::TypeScript
            | Self::Tsx
            | Self::Json
            | Self::Yaml
            | Self::Markdown
            | Self::Html
            | Self::Css
            | Self::Scss
            | Self::Less => Some(Formatter::Prettier),
            Self::Java => Some(Formatter::GoogleJavaFormat),
            Self::Kotlin => Some(Formatter::Ktlint),
            Self::Rust => Some(Formatter::Rustfmt),
            Self::Go => Some(Formatter::Gofmt),
            Self::Python => Some(Formatter::Black),
            Self::Toml | Self::Shell | Self::Sql | Self::Xml | Self::Dockerfile | Self::Other => None,
        }
    }
}

impl Formatter {
    /// The program looked up on `PATH`.
    pub fn program(self) -> &'static str {
        match self {
            Self::Prettier => "prettier",
            Self::GoogleJavaFormat => "google-java-format",
            Self::Ktlint => "ktlint",
            Self::Rustfmt => "rustfmt",
            Self::Gofmt => "gofmt",
            Self::Black => "black",
        }
    }

    /// How to install it, shown when it is missing (a command or a download page).
    pub fn install_hint(self) -> &'static str {
        match self {
            Self::Prettier => "npm install -g prettier",
            Self::GoogleJavaFormat if cfg!(target_os = "macos") => "brew install google-java-format",
            Self::GoogleJavaFormat => "https://github.com/google/google-java-format/releases",
            Self::Ktlint if cfg!(target_os = "macos") => "brew install ktlint",
            Self::Ktlint => "https://pinterest.github.io/ktlint/latest/install/cli/",
            Self::Rustfmt => "rustup component add rustfmt",
            Self::Gofmt => "https://go.dev/dl/",
            Self::Black => "pipx install black",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(name: &str) -> Language {
        Language::detect(Path::new(name))
    }

    #[test]
    fn languages_from_file_names() {
        assert_eq!(detect("src/Main.java"), Language::Java);
        assert_eq!(detect("build.gradle.kts"), Language::Kotlin);
        assert_eq!(detect("App.KT"), Language::Kotlin);
        assert_eq!(detect("index.mjs"), Language::JavaScript);
        assert_eq!(detect("view.jsx"), Language::Jsx);
        assert_eq!(detect("server.ts"), Language::TypeScript);
        assert_eq!(detect("Page.tsx"), Language::Tsx);
        assert_eq!(detect("package.json"), Language::Json);
        assert_eq!(detect("ci.yml"), Language::Yaml);
        assert_eq!(detect("Cargo.toml"), Language::Toml);
        assert_eq!(detect("Cargo.lock"), Language::Toml);
        assert_eq!(detect("README.md"), Language::Markdown);
        assert_eq!(detect("main.rs"), Language::Rust);
        assert_eq!(detect("app.py"), Language::Python);
        assert_eq!(detect("main.go"), Language::Go);
        assert_eq!(detect("install.sh"), Language::Shell);
        assert_eq!(detect(".zshrc"), Language::Shell);
        assert_eq!(detect("index.html"), Language::Html);
        assert_eq!(detect("site.css"), Language::Css);
        assert_eq!(detect("schema.sql"), Language::Sql);
        assert_eq!(detect("Dockerfile"), Language::Dockerfile);
        assert_eq!(detect("Dockerfile.dev"), Language::Dockerfile);
        assert_eq!(detect("notes.txt"), Language::Other);
        assert_eq!(detect("Makefile"), Language::Other);
    }

    #[test]
    fn formatters_per_language() {
        for language in [Language::JavaScript, Language::TypeScript, Language::Tsx, Language::Json, Language::Css, Language::Markdown] {
            assert_eq!(language.formatter(), Some(Formatter::Prettier), "{language:?}");
        }
        assert_eq!(Language::Java.formatter(), Some(Formatter::GoogleJavaFormat));
        assert_eq!(Language::Kotlin.formatter(), Some(Formatter::Ktlint));
        assert_eq!(Language::Rust.formatter(), Some(Formatter::Rustfmt));
        assert_eq!(Language::Go.formatter(), Some(Formatter::Gofmt));
        assert_eq!(Language::Python.formatter(), Some(Formatter::Black));
        assert_eq!(Language::Shell.formatter(), None);
        assert_eq!(Language::Other.formatter(), None);
    }
}
