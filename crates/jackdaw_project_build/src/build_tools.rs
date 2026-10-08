//! Native build tools a Bevy game build needs beyond Rust itself.
//!
//! Sys crates in a typical game compile C and C++ sources (jackdaw's CSG
//! kernel `manifold-csg-sys` with cmake), link against the platform's native
//! toolchain, and on Linux find Bevy's audio and input libraries through
//! pkg-config. Each gap otherwise surfaces minutes into a compile as a linker
//! or build-script error. [`check_build_tools`] reports them up front with a
//! fix per platform; the decision logic reads the machine only through a
//! [`ToolProbe`], so every platform's outcomes are testable anywhere.

use std::process::Command;

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// Good to go.
    Ok,
    /// Build can proceed but may misbehave; the fix is advisory.
    Warn,
    /// The build will fail until the fix is applied.
    Fail,
}

/// One build-tool check result.
#[derive(Debug, Clone)]
pub struct ToolCheck {
    /// Short name of the tool or package group.
    pub name: &'static str,
    /// Whether builds will work.
    pub status: CheckStatus,
    /// What was found, or what is missing.
    pub detail: String,
    /// How to fix a warning or failure.
    pub fix: Option<String>,
}

impl ToolCheck {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Ok,
            detail: detail.into(),
            fix: None,
        }
    }

    fn issue(
        name: &'static str,
        status: CheckStatus,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Operating system family the checks are chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows with the MSVC toolchain.
    Windows,
    /// macOS.
    MacOs,
    /// Linux.
    Linux,
    /// Any other Unix-like system.
    OtherUnix,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::OtherUnix
        }
    }
}

/// Read access to the machine for [`check_build_tools`].
pub trait ToolProbe {
    /// Run `cmd args`; on success, the first line of stdout (empty when it
    /// printed nothing). `None` when the command is missing or fails.
    fn run(&self, cmd: &str, args: &[&str]) -> Option<String>;
    /// The value of an environment variable.
    fn env(&self, var: &str) -> Option<String>;
}

/// [`ToolProbe`] that runs real commands and reads the real environment.
pub struct SystemProbe;

impl ToolProbe for SystemProbe {
    fn run(&self, cmd: &str, args: &[&str]) -> Option<String> {
        let out = jackdaw_env::without_console_window(&mut Command::new(cmd))
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        )
    }

    fn env(&self, var: &str) -> Option<String> {
        std::env::var(var).ok().filter(|value| !value.is_empty())
    }
}

/// Native build tools on this machine. Spawns processes; call off the main
/// thread in an interactive app.
pub fn check_build_tools() -> Vec<ToolCheck> {
    build_tool_checks(Platform::current(), &SystemProbe)
}

const BEVY_LINUX_DEPS: &str =
    "https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md";
const VS_BUILD_TOOLS: &str = "https://visualstudio.microsoft.com/visual-cpp-build-tools/";

/// The checks for `platform`, answered by `probe`.
pub fn build_tool_checks(platform: Platform, probe: &dyn ToolProbe) -> Vec<ToolCheck> {
    let mut out = vec![cmake(platform, probe)];
    match platform {
        Platform::Windows => {
            out.push(msvc(probe));
            if let Some(check) = mingw_shadowing_msvc(probe) {
                out.push(check);
            }
        }
        Platform::MacOs => out.push(xcode_command_line_tools(probe)),
        Platform::Linux => {
            out.push(unix_compilers(platform, probe));
            out.extend(linux_system_libraries(probe));
        }
        Platform::OtherUnix => out.push(unix_compilers(platform, probe)),
    }
    out
}

fn cmake(platform: Platform, probe: &dyn ToolProbe) -> ToolCheck {
    if let Some(version) = probe.run("cmake", &["--version"]) {
        return ToolCheck::ok("cmake", version);
    }
    let fix = match platform {
        Platform::Windows => {
            "Install cmake from https://cmake.org/download and choose the option that adds it to PATH"
        }
        Platform::MacOs => "Run brew install cmake, or install it from https://cmake.org/download",
        Platform::Linux | Platform::OtherUnix => {
            "Install cmake with your package manager, or from https://cmake.org/download"
        }
    };
    ToolCheck::issue(
        "cmake",
        CheckStatus::Fail,
        "cmake not found on PATH; jackdaw's CSG kernel is built with it",
        fix,
    )
}

/// The `--version` line of the first of `candidates` that answers it.
fn first_found(probe: &dyn ToolProbe, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .find_map(|cmd| probe.run(cmd, &["--version"]))
}

fn unix_compilers(platform: Platform, probe: &dyn ToolProbe) -> ToolCheck {
    // The cc crate honours CC and CXX first, then the system defaults.
    let candidates = |var: &str, defaults: &[&str]| {
        probe
            .env(var)
            .into_iter()
            .chain(defaults.iter().copied().map(String::from))
            .collect::<Vec<_>>()
    };
    let c = first_found(probe, &candidates("CC", &["cc", "clang", "gcc"]));
    let cxx = first_found(probe, &candidates("CXX", &["c++", "clang++", "g++"]));
    let missing = match (&c, &cxx) {
        (Some(_), Some(version)) => {
            return ToolCheck::ok("C/C++ compiler", version.clone());
        }
        (None, Some(_)) => "no C compiler found (tried cc, clang, gcc)",
        (Some(_), None) => "no C++ compiler found (tried c++, clang++, g++)",
        (None, None) => "no C or C++ compiler found (tried cc, c++, clang, gcc)",
    };
    let fix = if platform == Platform::Linux {
        "Install your distribution's compiler packages: build-essential (Debian, Ubuntu), \
         base-devel (Arch) or gcc-c++ (Fedora)"
    } else {
        "Install clang or gcc with C++ support from your package manager"
    };
    ToolCheck::issue("C/C++ compiler", CheckStatus::Fail, missing, fix)
}

fn xcode_command_line_tools(probe: &dyn ToolProbe) -> ToolCheck {
    match probe.run("xcode-select", &["-p"]) {
        Some(path) if !path.is_empty() => ToolCheck::ok("Xcode Command Line Tools", path),
        _ => ToolCheck::issue(
            "Xcode Command Line Tools",
            CheckStatus::Fail,
            "not installed; macOS builds need its C/C++ compiler and linker",
            "Run xcode-select --install in a terminal, then recheck",
        ),
    }
}

/// Where Visual Studio's C++ build tools are installed: the developer prompt's
/// `VCINSTALLDIR`, else what vswhere reports for the x64 VC tools component.
fn find_msvc(probe: &dyn ToolProbe) -> Option<String> {
    if let Some(dir) = probe.env("VCINSTALLDIR") {
        return Some(dir);
    }
    let program_files = probe
        .env("ProgramFiles(x86)")
        .or_else(|| probe.env("ProgramFiles"))?;
    let vswhere = format!("{program_files}\\Microsoft Visual Studio\\Installer\\vswhere.exe");
    probe
        .run(
            &vswhere,
            &[
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ],
        )
        .filter(|path| !path.is_empty())
}

fn msvc(probe: &dyn ToolProbe) -> ToolCheck {
    match find_msvc(probe) {
        Some(path) => ToolCheck::ok("MSVC build tools", path),
        None => ToolCheck::issue(
            "MSVC build tools",
            CheckStatus::Fail,
            "Visual Studio C++ build tools not found; Rust needs the MSVC linker (link.exe)",
            format!(
                "Install Visual Studio Build Tools with the Desktop development with C++ \
                 workload: {VS_BUILD_TOOLS}"
            ),
        ),
    }
}

/// A MinGW `gcc` on PATH makes cmake pick it over MSVC unless
/// `CMAKE_GENERATOR` names Visual Studio, and the objects then fail to link
/// (LNK1143).
fn mingw_shadowing_msvc(probe: &dyn ToolProbe) -> Option<ToolCheck> {
    probe.run("gcc", &["--version"])?;
    if probe
        .env("CMAKE_GENERATOR")
        .is_some_and(|generator| generator.contains("Visual Studio"))
    {
        return None;
    }
    Some(ToolCheck::issue(
        "MinGW gcc",
        CheckStatus::Warn,
        "MinGW gcc is on PATH; cmake may pick it over MSVC and fail to link (LNK1143)",
        "Set CMAKE_GENERATOR=\"Visual Studio 17 2022\" before building",
    ))
}

fn linux_system_libraries(probe: &dyn ToolProbe) -> Vec<ToolCheck> {
    let Some(version) = probe.run("pkg-config", &["--version"]) else {
        return vec![ToolCheck::issue(
            "pkg-config",
            CheckStatus::Warn,
            "pkg-config not found; Bevy's audio and input crates locate system libraries with it",
            format!(
                "Install pkg-config and Bevy's Linux dependencies (alsa, udev, wayland and x11 \
                 development packages): {BEVY_LINUX_DEPS}"
            ),
        )];
    };
    let missing: Vec<&str> = ["alsa", "libudev"]
        .into_iter()
        .filter(|lib| probe.run("pkg-config", &["--exists", lib]).is_none())
        .collect();
    let libraries = if missing.is_empty() {
        ToolCheck::ok("Bevy system libraries", "alsa and libudev found")
    } else {
        ToolCheck::issue(
            "Bevy system libraries",
            CheckStatus::Warn,
            format!(
                "{} development files not found; Bevy's default audio and gamepad support need them",
                missing.join(" and ")
            ),
            format!(
                "Install Bevy's Linux dependencies (alsa, udev, wayland and x11 development \
                 packages): {BEVY_LINUX_DEPS}"
            ),
        )
    };
    vec![ToolCheck::ok("pkg-config", version), libraries]
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// Answers only the commands and variables it was given.
    #[derive(Default)]
    struct FakeProbe {
        commands: HashMap<String, String>,
        env: HashMap<String, String>,
    }

    impl FakeProbe {
        fn with(mut self, command: &str, output: &str) -> Self {
            self.commands
                .insert(command.to_string(), output.to_string());
            self
        }

        fn with_env(mut self, var: &str, value: &str) -> Self {
            self.env.insert(var.to_string(), value.to_string());
            self
        }
    }

    impl ToolProbe for FakeProbe {
        fn run(&self, cmd: &str, args: &[&str]) -> Option<String> {
            let full = std::iter::once(cmd)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.commands
                .get(&full)
                .or_else(|| self.commands.get(cmd))
                .cloned()
        }

        fn env(&self, var: &str) -> Option<String> {
            self.env.get(var).cloned()
        }
    }

    fn status(checks: &[ToolCheck], name: &str) -> Option<CheckStatus> {
        checks
            .iter()
            .find(|check| check.name == name)
            .map(|check| check.status)
    }

    fn names(checks: &[ToolCheck]) -> HashSet<&'static str> {
        checks.iter().map(|check| check.name).collect()
    }

    fn healthy_linux() -> FakeProbe {
        FakeProbe::default()
            .with("cmake", "cmake version 3.30.0")
            .with("cc", "cc (GCC) 15.1")
            .with("c++", "c++ (GCC) 15.1")
            .with("pkg-config --version", "2.3.0")
            .with("pkg-config --exists alsa", "")
            .with("pkg-config --exists libudev", "")
    }

    #[test]
    fn healthy_linux_passes_every_check() {
        let checks = build_tool_checks(Platform::Linux, &healthy_linux());
        assert!(
            checks.iter().all(|check| check.status == CheckStatus::Ok),
            "{checks:?}"
        );
        assert_eq!(
            names(&checks),
            HashSet::from([
                "cmake",
                "C/C++ compiler",
                "pkg-config",
                "Bevy system libraries"
            ])
        );
    }

    #[test]
    fn missing_cmake_fails_on_every_platform() {
        for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
            let checks = build_tool_checks(platform, &FakeProbe::default());
            let cmake = checks.iter().find(|check| check.name == "cmake").unwrap();
            assert_eq!(cmake.status, CheckStatus::Fail);
            assert!(cmake.fix.as_deref().unwrap().contains("cmake.org"));
        }
    }

    #[test]
    fn a_missing_cpp_compiler_fails_on_unix() {
        let probe = FakeProbe::default().with("cc", "cc 15");
        let checks = build_tool_checks(Platform::Linux, &probe);
        let compiler = checks
            .iter()
            .find(|check| check.name == "C/C++ compiler")
            .unwrap();
        assert_eq!(compiler.status, CheckStatus::Fail);
        assert!(compiler.detail.contains("C++"), "{}", compiler.detail);
    }

    #[test]
    fn clang_alone_satisfies_the_unix_compiler_check() {
        let probe = FakeProbe::default()
            .with("clang", "clang version 20")
            .with("clang++", "clang version 20");
        let checks = build_tool_checks(Platform::OtherUnix, &probe);
        assert_eq!(status(&checks, "C/C++ compiler"), Some(CheckStatus::Ok));
    }

    #[test]
    fn cxx_from_the_environment_is_tried_first() {
        let probe = FakeProbe::default()
            .with_env("CC", "gcc-14")
            .with_env("CXX", "g++-14")
            .with("gcc-14", "gcc-14 14.2")
            .with("g++-14", "g++-14 14.2");
        let checks = build_tool_checks(Platform::Linux, &probe);
        let compiler = checks
            .iter()
            .find(|check| check.name == "C/C++ compiler")
            .unwrap();
        assert_eq!(compiler.status, CheckStatus::Ok);
        assert!(compiler.detail.starts_with("g++-14"), "{}", compiler.detail);
    }

    #[test]
    fn missing_linux_dev_packages_only_warn() {
        let probe = FakeProbe::default()
            .with("cmake", "cmake version 3.30.0")
            .with("cc", "cc 15")
            .with("c++", "c++ 15")
            .with("pkg-config --version", "2.3.0")
            .with("pkg-config --exists libudev", "");
        let checks = build_tool_checks(Platform::Linux, &probe);
        let libraries = checks
            .iter()
            .find(|check| check.name == "Bevy system libraries")
            .unwrap();
        assert_eq!(libraries.status, CheckStatus::Warn);
        assert!(
            libraries.detail.starts_with("alsa "),
            "{}",
            libraries.detail
        );
        assert!(libraries.fix.as_deref().unwrap().contains(BEVY_LINUX_DEPS));
    }

    #[test]
    fn missing_pkg_config_warns_and_skips_library_probes() {
        let probe = FakeProbe::default().with("cmake", "cmake version 3.30.0");
        let checks = build_tool_checks(Platform::Linux, &probe);
        assert_eq!(status(&checks, "pkg-config"), Some(CheckStatus::Warn));
        assert_eq!(status(&checks, "Bevy system libraries"), None);
    }

    #[test]
    fn macos_requires_the_command_line_tools() {
        let missing = build_tool_checks(Platform::MacOs, &FakeProbe::default());
        let clt = missing
            .iter()
            .find(|check| check.name == "Xcode Command Line Tools")
            .unwrap();
        assert_eq!(clt.status, CheckStatus::Fail);
        assert!(
            clt.fix
                .as_deref()
                .unwrap()
                .contains("xcode-select --install")
        );
        assert_eq!(status(&missing, "C/C++ compiler"), None);
        assert_eq!(status(&missing, "pkg-config"), None);

        let installed =
            FakeProbe::default().with("xcode-select -p", "/Library/Developer/CommandLineTools");
        let checks = build_tool_checks(Platform::MacOs, &installed);
        assert_eq!(
            status(&checks, "Xcode Command Line Tools"),
            Some(CheckStatus::Ok)
        );
    }

    #[test]
    fn windows_finds_msvc_through_vswhere() {
        let vswhere = "C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\vswhere.exe";
        let probe = FakeProbe::default()
            .with_env("ProgramFiles(x86)", "C:\\Program Files (x86)")
            .with(vswhere, "C:\\BuildTools");
        let checks = build_tool_checks(Platform::Windows, &probe);
        let msvc = checks
            .iter()
            .find(|check| check.name == "MSVC build tools")
            .unwrap();
        assert_eq!(msvc.status, CheckStatus::Ok);
        assert_eq!(msvc.detail, "C:\\BuildTools");
    }

    #[test]
    fn windows_developer_prompt_counts_as_msvc() {
        let probe = FakeProbe::default().with_env("VCINSTALLDIR", "C:\\VS\\VC\\");
        let checks = build_tool_checks(Platform::Windows, &probe);
        assert_eq!(status(&checks, "MSVC build tools"), Some(CheckStatus::Ok));
    }

    #[test]
    fn windows_without_msvc_fails_with_the_build_tools_link() {
        let probe = FakeProbe::default().with_env("ProgramFiles(x86)", "C:\\Program Files (x86)");
        let checks = build_tool_checks(Platform::Windows, &probe);
        let msvc = checks
            .iter()
            .find(|check| check.name == "MSVC build tools")
            .unwrap();
        assert_eq!(msvc.status, CheckStatus::Fail);
        let fix = msvc.fix.as_deref().unwrap();
        assert!(fix.contains("Desktop development with C++") && fix.contains(VS_BUILD_TOOLS));
        assert_eq!(status(&checks, "C/C++ compiler"), None);
    }

    #[test]
    fn windows_warns_only_on_unguarded_mingw() {
        let without_gcc = build_tool_checks(Platform::Windows, &FakeProbe::default());
        assert_eq!(status(&without_gcc, "MinGW gcc"), None);

        let gcc = FakeProbe::default().with("gcc", "gcc 14");
        let checks = build_tool_checks(Platform::Windows, &gcc);
        assert_eq!(status(&checks, "MinGW gcc"), Some(CheckStatus::Warn));

        let guarded = FakeProbe::default()
            .with("gcc", "gcc 14")
            .with_env("CMAKE_GENERATOR", "Visual Studio 17 2022");
        let checks = build_tool_checks(Platform::Windows, &guarded);
        assert_eq!(status(&checks, "MinGW gcc"), None);
    }
}
