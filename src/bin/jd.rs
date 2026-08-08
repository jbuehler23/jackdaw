//! Public Jackdaw command-line interface.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);
    if let Some(name) = command
        && let Some(spec) = SPECS.iter().find(|spec| spec.name == name)
    {
        match check_options(spec, &args[1..]) {
            OptionCheck::Ok => {}
            OptionCheck::Help => {
                print_command_usage(spec);
                return ExitCode::SUCCESS;
            }
            OptionCheck::Rejected(message) => {
                report_option_error(spec, &message);
                return ExitCode::FAILURE;
            }
        }
    }
    match command {
        Some("new") => exit(jackdaw::scaffold::run_new_cli(&args[1..])),
        Some("import") => exit(jackdaw::scaffold::run_import_cli(&args[1..])),
        Some("open") => exit(jackdaw::scaffold::run_open_cli(&args[1..])),
        Some("upgrade") => exit(jackdaw::scaffold::run_upgrade_cli(&args[1..])),
        Some("extension") => extension_command(&args[1..]),
        Some("--help" | "-h" | "help") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some("build" | "run" | "setup" | "doctor" | "--version" | "-V" | "version") => {
            jackdaw_cli_internal::run_args(&args)
        }
        Some(other) => {
            unknown_command(other);
            ExitCode::FAILURE
        }
    }
}

/// One subcommand's accepted options, for rejecting the rest.
///
/// Unrecognised options used to be ignored, so `jd new x --ext` (a typo
/// for `--extension`) silently produced a game project and `jd new x
/// --path` with no value created it in the working directory. A user
/// only discovers either much later.
struct CommandSpec {
    name: &'static str,
    /// Options that stand alone.
    flags: &'static [&'static str],
    /// Options that consume the following argument.
    values: &'static [&'static str],
    usage: &'static str,
}

const SPECS: &[CommandSpec] = &[
    CommandSpec {
        name: "new",
        flags: &["--extension", "--no-git"],
        values: &["--path"],
        usage: "jd new <name> [--extension] [--path <dir>] [--no-git]",
    },
    CommandSpec {
        name: "import",
        flags: &["--apply", "--allow-bevy-mismatch"],
        values: &["--package", "--plugin"],
        usage: "jd import [path] [--apply] [--package <name>] [--plugin <Type>] \
                [--allow-bevy-mismatch]",
    },
    CommandSpec {
        name: "open",
        flags: &[],
        values: &[],
        usage: "jd open [path]",
    },
    CommandSpec {
        name: "upgrade",
        flags: &["--apply"],
        values: &[],
        usage: "jd upgrade [path] [--apply]",
    },
    CommandSpec {
        name: "build",
        flags: &[],
        values: &["--project", "-p"],
        usage: "jd build [--project <path>]",
    },
    CommandSpec {
        name: "run",
        flags: &[],
        values: &["--project", "-p"],
        usage: "jd run [--project <path>]",
    },
    CommandSpec {
        name: "setup",
        flags: &[],
        values: &[],
        usage: "jd setup",
    },
    CommandSpec {
        name: "doctor",
        flags: &[],
        values: &["--project", "-p"],
        usage: "jd doctor [--project <path>]",
    },
];

enum OptionCheck {
    Ok,
    Help,
    Rejected(String),
}

fn check_options(spec: &CommandSpec, args: &[String]) -> OptionCheck {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--help" || arg == "-h" {
            return OptionCheck::Help;
        }
        if spec.values.contains(&arg) {
            // A missing value silently shifted the meaning of the rest
            // of the line, so require one explicitly.
            match args.get(index + 1) {
                Some(value) if !value.starts_with('-') => index += 2,
                _ => return OptionCheck::Rejected(format!("{arg} needs a value")),
            }
            continue;
        }
        if arg.starts_with('-') && !spec.flags.contains(&arg) {
            return OptionCheck::Rejected(format!("unknown option `{arg}`"));
        }
        index += 1;
    }
    OptionCheck::Ok
}

#[expect(clippy::print_stdout, reason = "CLI help is written to stdout")]
fn print_command_usage(spec: &CommandSpec) {
    println!("usage: {}", spec.usage);
}

#[expect(clippy::print_stderr, reason = "CLI errors are written to stderr")]
fn report_option_error(spec: &CommandSpec, message: &str) {
    eprintln!("jd {}: {message}", spec.name);
    eprintln!("usage: {}", spec.usage);
}

/// Name the real command list. The fallthrough used to reach a second,
/// older usage string that listed neither `new` nor `import`, which is
/// the worst possible moment to hide them.
#[expect(clippy::print_stderr, reason = "CLI errors are written to stderr")]
fn unknown_command(name: &str) {
    eprintln!("jd: unknown command `{name}`\n");
    print_usage();
}

fn exit(value: bevy::app::AppExit) -> ExitCode {
    match value {
        bevy::app::AppExit::Success => ExitCode::SUCCESS,
        bevy::app::AppExit::Error(code) => ExitCode::from(code.get()),
    }
}

#[expect(clippy::print_stdout, reason = "CLI help is written to stdout")]
fn print_usage() {
    println!(
        "usage: jd <command>\n\n\
         commands:\n  \
         new <name>                 Create a Jackdaw project\n    \
           --extension                start from the editor-extension template\n    \
           --path <dir>               create it under <dir> instead of here\n    \
           --no-git                   skip initialising a git repository\n  \
         import [path]              Preview integration for an existing Bevy project\n    \
           --apply                    write the previewed changes\n    \
           --package <name>           in a workspace, which member is the game\n    \
           --plugin <Type>            the game plugin type to record\n    \
           --allow-bevy-mismatch      set up even if the project targets another Bevy\n  \
         open [path]                Open a project in the Jackdaw GUI\n  \
         upgrade [path] [--apply]   Move a project to this Jackdaw's versions\n  \
         build [--project <path>]   Build project code for the editor\n  \
         run [--project <path>]     Build and run the game\n  \
         setup                      Prepare the pinned SDK\n  \
         doctor [--project <path>]  Check build prerequisites and a project's setup\n  \
         extension <command>        Package or manage signed extensions\n\n\
         Start here: `jd new my-game` then `jd open my-game`, or `jd import .` in an \
         existing Bevy project."
    );
}

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "extension commands report through the terminal"
)]
fn extension_command(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("keygen") => {
            // Defaulting to a well-known location means a publisher runs
            // `keygen` once and never passes `--key` again.
            let path = match args.get(1) {
                Some(path) => std::path::PathBuf::from(path),
                None => match default_publisher_key() {
                    Some(path) => path,
                    None => {
                        eprintln!("usage: jd extension keygen <publisher-key.pk8>");
                        return ExitCode::FAILURE;
                    }
                },
            };
            if path.exists() {
                eprintln!(
                    "jd extension keygen: {} already exists; publishing updates under a new key \
                     forces every user to make the trust decision again, so this is never \
                     overwritten. Delete it deliberately if you really mean to rotate.",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match jackdaw_loader::package::generate_signing_key()
                .and_then(|bytes| write_private_key(&path, &bytes))
            {
                Ok(()) => {
                    println!("generated publisher key at {}", path.display());
                    println!("Keep it private and reuse it for every release of your extensions.");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("jd extension keygen: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("pack") => pack_extension(&args[1..]),
        Some("install") => {
            let Some(source) = args.iter().skip(1).find(|arg| !arg.starts_with('-')) else {
                eprintln!(
                    "usage: jd extension install <bundle.jdext|https://...> [--trust-publisher]"
                );
                return ExitCode::FAILURE;
            };
            let decision = if args.iter().any(|arg| arg == "--trust-publisher") {
                jackdaw_loader::package::TrustDecision::TrustPublisher
            } else {
                jackdaw_loader::package::TrustDecision::RequireTrusted
            };
            // A URL installs the same way a file does: fetch, then hand
            // the bytes to the same signature/ABI/trust gate. Without
            // this, every marketplace has to tell users to download a
            // file first and then find it again.
            let bytes = match read_bundle_source(source) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("jd extension install: {error}");
                    return ExitCode::FAILURE;
                }
            };
            match jackdaw_loader::package::install_bytes(&bytes, decision) {
                Ok(installed) => {
                    println!(
                        "installed {} {} at {}",
                        installed.manifest.id,
                        installed.manifest.version,
                        installed.library_path.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("jd extension install: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        // What a marketplace needs to serve the right artifact: the
        // exact compatibility key this build will accept.
        Some("abi") => {
            if args.iter().any(|arg| arg == "--json") {
                println!(
                    "{{\"sdk_abi\":{},\"target\":{},\"jackdaw\":{},\"bevy\":{}}}",
                    json_string(&jackdaw_loader::package::host_sdk_abi()),
                    json_string(jackdaw_loader::package::host_target()),
                    json_string(jackdaw_project_build::VERSION),
                    json_string(jackdaw_project_build::BEVY_VERSION),
                );
            } else {
                println!("sdk_abi: {}", jackdaw_loader::package::host_sdk_abi());
                println!("target:  {}", jackdaw_loader::package::host_target());
                println!(
                    "jackdaw: {} (bevy {})",
                    jackdaw_project_build::VERSION,
                    jackdaw_project_build::BEVY_VERSION
                );
            }
            ExitCode::SUCCESS
        }
        Some("list") => match jackdaw_loader::package::list_installed() {
            Ok(installed) => {
                for extension in installed {
                    println!(
                        "{}\t{}\t{}\t{}",
                        extension.manifest.id,
                        extension.manifest.version,
                        extension.manifest.publisher,
                        extension.library_path.display()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("jd extension list: {error}");
                ExitCode::FAILURE
            }
        },
        Some("verify") => {
            let Some(path) = args.get(1) else {
                eprintln!("usage: jd extension verify <bundle.jdext>");
                return ExitCode::FAILURE;
            };
            match jackdaw_loader::package::verify_bundle(std::path::Path::new(path)) {
                Ok(bundle) => {
                    println!(
                        "verified {} {} from {} for {}",
                        bundle.manifest.id,
                        bundle.manifest.version,
                        bundle.manifest.publisher,
                        bundle.manifest.target
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("jd extension verify: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("enable" | "disable") => {
            let enabled = args[0] == "enable";
            let Some(id) = args.get(1) else {
                eprintln!("usage: jd extension {} <id>", args[0]);
                return ExitCode::FAILURE;
            };
            jackdaw_api_internal::extensions_config::set_extension_enabled(id, enabled);
            println!("{} {}", if enabled { "enabled" } else { "disabled" }, id);
            ExitCode::SUCCESS
        }
        Some("uninstall") => {
            let Some(id) = args.get(1) else {
                eprintln!("usage: jd extension uninstall <id>");
                return ExitCode::FAILURE;
            };
            match jackdaw_loader::package::uninstall(id) {
                Ok(true) => {
                    println!("uninstalled {id}; retired files will be removed next launch");
                    ExitCode::SUCCESS
                }
                Ok(false) => {
                    eprintln!("jd extension uninstall: `{id}` is not installed");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("jd extension uninstall: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!(
                "usage: jd extension <command>\n\n\
                 commands:\n  \
                 keygen [path]              Create the publisher signing key (once)\n  \
                 pack [--project <dir>]     Build a signed bundle from the project's manifest\n  \
                 install <bundle|https url> Install a bundle (--trust-publisher to accept a new key)\n  \
                 abi [--json]               Print the compatibility key this build accepts\n  \
                 verify <bundle.jdext>      Check a bundle's signature and ABI without installing\n  \
                 list                       List installed extensions\n  \
                 enable|disable <id>        Toggle an installed extension\n  \
                 uninstall <id>             Remove an installed extension\n\n\
                 Publishing: `jd extension keygen` once, then `jd build && jd extension pack` \
                 per release."
            );
            ExitCode::FAILURE
        }
    }
}

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "extension packaging reports through the terminal"
)]
fn pack_extension(args: &[String]) -> ExitCode {
    // Every field has a source: the extension project's own Cargo.toml.
    // Flags stay available to override any of them, but the intended
    // publishing flow is `jd build && jd extension pack` in the project.
    let project = match resolve_pack_project(args) {
        Ok(project) => project,
        Err(message) => {
            eprintln!("jd extension pack: {message}");
            eprintln!(
                "\nusage: jd extension pack [--project <dir>] [--library <path>]\n\
                 \n\
                 Metadata defaults to the project's Cargo.toml; override any of\n\
                 --id --label --version --publisher --license --homepage --key --out."
            );
            return ExitCode::FAILURE;
        }
    };

    let Some(library_path) = flag_value(args, "--library")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            project
                .root
                .as_deref()
                .and_then(|root| jackdaw_project_build::last_built_dylib(&root.join(".jackdaw")))
        })
        .or_else(|| first_positional(args).map(std::path::PathBuf::from))
    else {
        eprintln!(
            "jd extension pack: no built library found. Run `jd build` in the extension \
             project first, or pass --library <path>."
        );
        return ExitCode::FAILURE;
    };

    let field = |flag: &str, derived: Option<String>| flag_value(args, flag).or(derived);
    let Some(id) = field("--id", project.name.clone()) else {
        return missing_pack_flag("--id");
    };
    let Some(version) = field("--version", project.version.clone()) else {
        return missing_pack_flag("--version");
    };
    let Some(publisher) = field("--publisher", project.publisher.clone()) else {
        eprintln!(
            "jd extension pack: no publisher found. Add `authors = [\"You <you@example.com>\"]` \
             to Cargo.toml, or pass --publisher."
        );
        return ExitCode::FAILURE;
    };
    let Some(license) = field("--license", project.license.clone()) else {
        eprintln!(
            "jd extension pack: no license found. Add `license = \"MIT OR Apache-2.0\"` to \
             Cargo.toml, or pass --license."
        );
        return ExitCode::FAILURE;
    };
    let label = field("--label", project.label.clone()).unwrap_or_else(|| id.clone());
    let Some(key_path) = flag_value(args, "--key")
        .map(std::path::PathBuf::from)
        .or_else(default_publisher_key)
        .filter(|path| path.is_file())
    else {
        eprintln!(
            "jd extension pack: no signing key. Run `jd extension keygen` once, or pass \
             --key <publisher-key.pk8>."
        );
        return ExitCode::FAILURE;
    };
    let out_path = flag_value(args, "--out").unwrap_or_else(|| {
        format!(
            "{id}-{version}-{}.jdext",
            jackdaw_loader::package::host_target()
        )
    });

    let library_name = library_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let result = (|| {
        let library = std::fs::read(&library_path).map_err(|error| {
            jackdaw_loader::package::PackageError::Io(format!(
                "{}: {error}",
                library_path.display()
            ))
        })?;
        let key = std::fs::read(&key_path).map_err(|error| {
            jackdaw_loader::package::PackageError::Io(format!("{}: {error}", key_path.display()))
        })?;
        let mut manifest = jackdaw_loader::package::ExtensionManifest::new(
            &id,
            label,
            &version,
            publisher,
            license,
            library_name,
            &library,
        );
        manifest.homepage = flag_value(args, "--homepage").or(project.homepage.clone());
        let bundle = jackdaw_loader::package::create_bundle(manifest, &library, &key)?;
        std::fs::write(&out_path, bundle).map_err(|error| {
            jackdaw_loader::package::PackageError::Io(format!("{out_path}: {error}"))
        })
    })();

    match result {
        Ok(()) => {
            println!("packed {id} {version} at {out_path}");
            println!("  library: {}", library_path.display());
            println!(
                "  built for: {} / {}",
                jackdaw_loader::package::host_target(),
                jackdaw_loader::package::host_sdk_abi()
            );
            println!(
                "\nA bundle only installs into a jackdaw with the same ABI string, so publish \
                 one per target and jackdaw release."
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("jd extension pack: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Publishing metadata read out of an extension project's manifest.
#[derive(Default)]
struct PackProject {
    root: Option<std::path::PathBuf>,
    name: Option<String>,
    label: Option<String>,
    version: Option<String>,
    publisher: Option<String>,
    license: Option<String>,
    homepage: Option<String>,
}

/// Read packing metadata from `--project` (or the working directory when
/// it is a cargo project). Not being in a project is fine as long as the
/// flags supply everything; it is only an error when the requested
/// directory does not exist.
fn resolve_pack_project(args: &[String]) -> Result<PackProject, String> {
    let requested = flag_value(args, "--project").map(std::path::PathBuf::from);
    let explicit = requested.is_some();
    let Some(root) = requested
        .or_else(|| std::env::current_dir().ok())
        .and_then(|dir| dunce::canonicalize(dir).ok())
        .filter(|dir| explicit || dir.join("Cargo.toml").is_file())
    else {
        return if explicit {
            Err("the requested --project directory does not exist".to_string())
        } else {
            Ok(PackProject::default())
        };
    };

    let manifest_path = root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest_path) else {
        return Err(format!("no Cargo.toml at {}", root.display()));
    };
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("could not parse {}: {error}", manifest_path.display()))?;
    let package = doc.get("package");
    let string = |key: &str| {
        package
            .and_then(|p| p.get(key))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let metadata = |key: &str| {
        package
            .and_then(|p| p.get("metadata"))
            .and_then(|m| m.get("jackdaw"))
            .and_then(|j| j.get(key))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    Ok(PackProject {
        name: string("name"),
        label: metadata("label"),
        version: string("version"),
        publisher: metadata("publisher").or_else(|| {
            package
                .and_then(|p| p.get("authors"))
                .and_then(|value| value.as_array())
                .and_then(|authors| authors.iter().next())
                .and_then(|author| author.as_str())
                .map(str::to_string)
                .filter(|author| !author.trim().is_empty())
        }),
        license: string("license"),
        homepage: string("homepage").or_else(|| string("repository")),
        root: Some(root),
    })
}

/// The conventional publisher key location, so `keygen` and `pack` agree
/// without either being told where it is.
fn default_publisher_key() -> Option<std::path::PathBuf> {
    jackdaw_project_build::bootstrap::data_dir().map(|dir| dir.join("publisher-key.pk8"))
}

fn first_positional(args: &[String]) -> Option<&String> {
    let value_flags = [
        "--project",
        "--library",
        "--id",
        "--label",
        "--version",
        "--publisher",
        "--license",
        "--homepage",
        "--key",
        "--out",
    ];
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if value_flags.contains(&arg.as_str()) {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// Read a bundle from a local path or an `https://` URL.
///
/// Only the bytes differ: verification, the ABI and target gate, and
/// the publisher trust prompt all run identically afterwards, so a
/// remote install is never less checked than a local one.
fn read_bundle_source(source: &str) -> Result<Vec<u8>, String> {
    if !(source.starts_with("https://") || source.starts_with("http://")) {
        return std::fs::read(source).map_err(|error| format!("{source}: {error}"));
    }
    // Plain http would let a network attacker choose the bytes. The
    // signature check would still reject an unsigned substitute, but
    // only after downloading it, and a marketplace has no reason to
    // serve one.
    if source.starts_with("http://") {
        return Err(format!(
            "{source}: refusing to fetch a bundle over plain http; use https"
        ));
    }
    let response = ehttp::fetch_blocking(&ehttp::Request::get(source))
        .map_err(|error| format!("{source}: {error}"))?;
    if !response.ok {
        return Err(format!(
            "{source}: server returned {} {}",
            response.status, response.status_text
        ));
    }
    Ok(response.bytes)
}

/// Minimal JSON string escaping, for the handful of fields `abi --json`
/// emits. Pulling in a serializer for four known-shaped values would be
/// more machinery than the output.
fn json_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

#[expect(
    clippy::print_stderr,
    reason = "extension packaging reports through the terminal"
)]
fn missing_pack_flag(flag: &str) -> ExitCode {
    eprintln!("jd extension pack: {flag} is required");
    ExitCode::FAILURE
}

#[cfg(unix)]
fn write_private_key(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), jackdaw_loader::package::PackageError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).map_err(|error| {
        jackdaw_loader::package::PackageError::Io(format!("{}: {error}", path.display()))
    })?;
    std::io::Write::write_all(&mut file, bytes).map_err(|error| {
        jackdaw_loader::package::PackageError::Io(format!("{}: {error}", path.display()))
    })
}

#[cfg(not(unix))]
fn write_private_key(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), jackdaw_loader::package::PackageError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(path).map_err(|error| {
        jackdaw_loader::package::PackageError::Io(format!("{}: {error}", path.display()))
    })?;
    std::io::Write::write_all(&mut file, bytes).map_err(|error| {
        jackdaw_loader::package::PackageError::Io(format!("{}: {error}", path.display()))
    })
}
