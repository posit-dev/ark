//
// main.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(unused_unsafe)]

use std::cell::Cell;
use std::env;

use amalthea::kernel;
use amalthea::kernel_spec::KernelSpec;
use ark::interface::SessionMode;
use ark::logger;
use ark::repos::DefaultRepos;
use ark::signals::initialize_signal_block;
use ark::start::start_kernel;
use ark::traps::register_trap_handlers;
use ark::version::detect_r;
use crossbeam::channel::unbounded;
use notify::Watcher;
use stdext::unwrap;

thread_local! {
    pub static ON_R_THREAD: Cell<bool> = Cell::new(false);
}

fn print_usage() {
    println!("Ark {}, an R Kernel.", env!("CARGO_PKG_VERSION"));
    println!(
        r#"
Usage: ark [OPTIONS]

Available options:

--connection_file FILE   Start the kernel with the given JSON connection file
                         (see the Jupyter kernel documentation for details)
-- arg1 arg2 ...         Set the argument list to pass to R; defaults to
                         --interactive
--startup-file FILE      An R file to run on session startup
--session-mode MODE      The mode in which the session is running (console, notebook, background)
--no-capture-streams     Do not capture stdout/stderr from R
--default-repos          Set the default repositories to use:
                         "rstudio" ('cran.rstudio.com', the default), or
                         "posit-ppm" ('packagemanager.posit.co', subject to availability), or
                         a path to a .conf file containing a list of named repositories (`name = url`), or
                         "none" (do not alter the 'repos' option in any way)
--version                Print the version of Ark
--log FILE               Log to the given file (if not specified, stdout/stderr
                         will be used)
--install                Install the kernel spec for Ark
--help                   Print this help message
"#
    );
}

fn main() -> anyhow::Result<()> {
    ON_R_THREAD.set(true);

    // Block signals in this thread (and any child threads).
    initialize_signal_block();

    // Get an iterator over all the command-line arguments
    let mut argv = std::env::args();

    // Skip the first "argument" as it's the path/name to this executable
    argv.next();

    let mut connection_file: Option<String> = None;
    let mut startup_file: Option<String> = None;
    let mut session_mode = SessionMode::Console;
    let mut log_file: Option<String> = None;
    let mut profile_file: Option<String> = None;
    let mut startup_notifier_file: Option<String> = None;
    let mut startup_delay: Option<std::time::Duration> = None;
    let mut r_args: Vec<String> = Vec::new();
    let mut has_action = false;
    let mut capture_streams = true;
    let mut default_repos = DefaultRepos::Auto;

    // Process remaining arguments. TODO: Need an argument that can passthrough args to R
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--connection_file" => {
                if let Some(file) = argv.next() {
                    connection_file = Some(file);
                    has_action = true;
                } else {
                    return Err(anyhow::anyhow!(
                        "A connection file must be specified when using the `--connection_file` argument."
                    ));
                }
            },
            "--startup-file" => {
                if let Some(file) = argv.next() {
                    startup_file = Some(file);
                    has_action = true;
                } else {
                    return Err(anyhow::anyhow!(
                        "A startup file must be specified when using the `--startup-file` argument."
                    ));
                }
            },
            "--session-mode" => {
                if let Some(mode) = argv.next() {
                    session_mode = match mode.as_str() {
                        "console" => SessionMode::Console,
                        "notebook" => SessionMode::Notebook,
                        "background" => SessionMode::Background,
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Invalid session mode: '{mode}'. Expected `console`, `notebook`, or `background`."
                            ));
                        },
                    };
                } else {
                    return Err(anyhow::anyhow!(
                        "A session mode must be specified when using the `--session-mode` argument."
                    ));
                }
            },
            "--version" => {
                println!("Ark {}", env!("CARGO_PKG_VERSION"));
                has_action = true;
            },
            "--install" => {
                install_kernel_spec()?;
                has_action = true;
            },
            "--help" => {
                print_usage();
                has_action = true;
            },
            "--no-capture-streams" => capture_streams = false,
            "--default-repos" => {
                if let Some(repos) = argv.next() {
                    default_repos = match repos.as_str() {
                        "rstudio" => DefaultRepos::RStudio,
                        "posit-ppm" => DefaultRepos::PositPPM,
                        "none" => DefaultRepos::None,
                        _ => {
                            // If the string is not one of the predefined options, assume it's a
                            // file path
                            let path = std::path::PathBuf::from(repos.clone());

                            // Check to see if the file exists
                            if !path.exists() {
                                return Err(anyhow::anyhow!(
                                    "The specified default repository configuration file {repos:?} does not exist."
                                ));
                            }
                            DefaultRepos::ConfFile(path)
                        },
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "A default repository must follow the --default-repos option; valid values are 'rstudio', 'posit-ppm', 'none', or a path to a .conf file."
                    ));
                }
            },
            "--log" => {
                if let Some(file) = argv.next() {
                    log_file = Some(file);
                } else {
                    return Err(anyhow::anyhow!(
                        "A log file must be specified when using the `--log` argument."
                    ));
                }
            },
            "--profile" => {
                if let Some(file) = argv.next() {
                    profile_file = Some(file);
                } else {
                    return Err(anyhow::anyhow!(
                        "A profile file must be specified when using the `--profile` argument."
                    ));
                }
            },
            "--startup-notifier-file" => {
                if let Some(file) = argv.next() {
                    startup_notifier_file = Some(file);
                } else {
                    return Err(anyhow::anyhow!(
                        "A notification file must be specified when using the `--startup-notifier-file` argument."
                    ));
                }
            },
            "--startup-delay" => {
                if let Some(delay_arg) = argv.next() {
                    if let Ok(delay) = delay_arg.parse::<u64>() {
                        startup_delay = Some(std::time::Duration::from_secs(delay));
                    } else {
                        return Err(anyhow::anyhow!("Can't parse delay in seconds"));
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "A delay in seconds must be specified when using the `--startup-delay` argument."
                    ));
                }
            },
            "--" => {
                // Consume the rest of the arguments for passthrough delivery to R
                while let Some(arg) = argv.next() {
                    r_args.push(arg);
                }
                break;
            },
            other => {
                return Err(anyhow::anyhow!("Argument '{other}' unknown."));
            },
        }
    }

    // Initialize the logger.
    logger::init(log_file.as_deref(), profile_file.as_deref());

    if let Some(file) = startup_notifier_file {
        let path = std::path::Path::new(&file);
        let (tx, rx) = unbounded();

        if let Err(err) = (|| -> anyhow::Result<()> {
            let config = notify::Config::default()
                .with_poll_interval(std::time::Duration::from_millis(2))
                .with_compare_contents(false);

            let handler = move |x| {
                let _ = tx.send(x);
            };
            let mut watcher = notify::RecommendedWatcher::new(handler, config).unwrap();
            watcher.watch(path, notify::RecursiveMode::NonRecursive)?;

            loop {
                let ev = rx.recv()?;
                match ev.unwrap().kind {
                    notify::event::EventKind::Modify(_) => {
                        break;
                    },
                    notify::event::EventKind::Remove(_) => {
                        break;
                    },
                    _ => {
                        continue;
                    },
                }
            }

            watcher.unwatch(path)?;
            Ok(())
        })() {
            eprintln!("Problem with the delay file: {:?}", err);
        }
    }

    if let Some(delay) = startup_delay {
        std::thread::sleep(delay);
    }

    // If the user didn't specify an action, print the usage instructions and
    // exit
    if !has_action {
        print_usage();
        return Ok(());
    }

    // Register segfault handler to get a backtrace. Should be after
    // initialising `log!`. Note that R will not override this handler
    // because we set `R_SignalHandlers` to 0 before startup.
    register_trap_handlers();

    // If the r_args vector is empty, add `--interactive` to the list of
    // arguments to pass to R.
    if r_args.is_empty() {
        r_args.push(String::from("--interactive"));
    }

    // This causes panics on background threads to propagate on the main
    // thread. If we don't propagate a background thread panic, the program
    // keeps running in an unstable state as all communications with this
    // thread will error out or panic.
    // https://stackoverflow.com/questions/35988775/how-can-i-cause-a-panic-on-a-thread-to-immediately-end-the-main-thread
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let info = panic_info.payload();

        let loc = if let Some(location) = panic_info.location() {
            format!("In file '{}' at line {}:", location.file(), location.line(),)
        } else {
            String::from("No location information:")
        };

        let append_trace = |info: &str| -> String {
            // Top-level-exec and try-catch errors already contain a backtrace
            // for the R thread so don't repeat it if we see one. Only perform
            // this check on the R thread because we do want other threads'
            // backtraces if the panic occurred elsewhere.
            if ON_R_THREAD.get() && info.contains("\n{R_BACKTRACE_HEADER}\n") {
                String::from("")
            } else {
                format!(
                    "\n\nBacktrace:\n{}",
                    std::backtrace::Backtrace::force_capture()
                )
            }
        };

        // Report panic to the frontend
        if let Some(info) = info.downcast_ref::<&str>() {
            let trace = append_trace(info);
            log::error!("Panic! {loc} {info:}{trace}");
        } else if let Some(info) = info.downcast_ref::<String>() {
            let trace = append_trace(&info);
            log::error!("Panic! {loc} {info:}{trace}");
        } else {
            let trace = format!("Backtrace:\n{}", std::backtrace::Backtrace::force_capture());
            log::error!("Panic! {loc} No contextual information.\n{trace}");
        }

        // Give some time to flush log
        log::logger().flush();
        std::thread::sleep(std::time::Duration::from_millis(250));

        old_hook(panic_info);
        std::process::abort();
    }));

    let Some(connection_file) = connection_file else {
        return Err(anyhow::anyhow!(
            "A connection file must be specified. Use the `--connection_file` argument."
        ));
    };

    // Parse the connection file
    let (connection_file, registration_file) = kernel::read_connection(connection_file.as_str());

    // Connect the Jupyter kernel and start R.
    // Does not return!
    start_kernel(
        connection_file,
        registration_file,
        r_args,
        startup_file,
        session_mode,
        capture_streams,
        default_repos,
    );

    // Just to please Rust
    Ok(())
}

// Install the kernelspec JSON file into one of Jupyter's search paths.
fn install_kernel_spec() -> anyhow::Result<()> {
    // Create the environment set for the kernel spec
    let mut env = serde_json::Map::new();

    // Workaround for https://github.com/posit-dev/positron/issues/2098
    env.insert("RUST_LOG".into(), serde_json::Value::String("error".into()));

    // Point `LD_LIBRARY_PATH` to a folder with some `libR.so`. It doesn't
    // matter which one, but the linker needs to be able to find a file of that
    // name, even though we won't use it for symbol resolution.
    // https://github.com/posit-dev/positron/issues/1619#issuecomment-1971552522
    if cfg!(target_os = "linux") {
        // Detect the active version of R
        let r_version = detect_r().unwrap();

        let lib = format!("{}/lib", r_version.r_home.clone());
        env.insert("LD_LIBRARY_PATH".into(), serde_json::Value::String(lib));
    }

    // Create the kernelspec
    let exe_path = unwrap!(env::current_exe(), Err(error) => {
        return Err(anyhow::anyhow!("Failed to determine path to Ark. {error:?}"));
    });

    let spec = KernelSpec {
        argv: vec![
            String::from(exe_path.to_string_lossy()),
            String::from("--connection_file"),
            String::from("{connection_file}"),
            String::from("--session-mode"),
            String::from("notebook"),
        ],
        language: String::from("R"),
        display_name: String::from("Ark R Kernel"),
        env,
    };

    let dest = unwrap!(spec.install(String::from("ark")), Err(err) => {
        return Err(anyhow::anyhow!("Failed to install Ark's Jupyter kernelspec. {err}"))
    });

    println!(
        "Successfully installed Ark Jupyter kernelspec.

    Kernel: {}
    ",
        dest.to_string_lossy()
    );

    Ok(())
}
