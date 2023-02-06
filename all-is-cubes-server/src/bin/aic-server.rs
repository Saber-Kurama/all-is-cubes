//! main() for a server that serves the All is Cubes client as well as being a game
//! server.

// Basic lint settings, which should be identical across all all-is-cubes project crates.
// This list is sorted.
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::needless_update)]
#![allow(clippy::single_match)]
#![deny(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::cast_lossless)]
#![warn(clippy::doc_markdown)]
#![warn(clippy::exhaustive_enums)]
#![warn(clippy::exhaustive_structs)]
#![warn(clippy::modulo_arithmetic)]
#![warn(clippy::return_self_not_must_use)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::uninlined_format_args)]
#![warn(clippy::unnecessary_self_imports)]
#![warn(clippy::wrong_self_convention)]
#![warn(explicit_outlives_requirements)]
#![warn(missing_debug_implementations)]
#![warn(noop_method_call)]
#![warn(trivial_numeric_casts)]
#![warn(unused_extern_crates)]
#![warn(unused_lifetimes)]
// Lenience for tests.
#![cfg_attr(test,
    allow(clippy::float_cmp), // deterministic tests
    allow(clippy::redundant_clone), // prefer regularity over efficiency
)]
// #![warn(unused_crate_dependencies)]  // noisy for dev-dependencies; enable sometimes for review

// Crate-specific lint settings.
#![forbid(unsafe_code)]

use all_is_cubes_server::webserver::start_server;

#[derive(Debug, clap::Parser)]
struct Args {
    #[arg(long, short = 'v')]
    verbose: bool,

    /// TCP port to bind.
    ///
    /// If not specified, an arbitrary port will be chosen.
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let Args { port, verbose } = <Args as clap::Parser>::parse();

    // Note: Something like this log configuration also appears in other binaries.
    // Unclear how to deduplicate since we don't want to have a library-level dep on
    // simplelog. For now, just remember to consider updating other instances.
    use simplelog::LevelFilter::{Debug, Off, Trace};
    simplelog::TermLogger::init(
        match verbose {
            false => Debug,
            true => Trace,
        },
        simplelog::ConfigBuilder::new()
            .set_target_level(Trace)
            .set_location_level(Off)
            .add_filter_ignore_str("tracing") // noisy
            .add_filter_ignore_str("hyper") // noisy
            .add_filter_ignore_str("mio") // uninteresting
            .build(),
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Auto,
    )?;

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(0)));

    let (url, finished) = start_server(addr)?;
    println!("{url}"); // note: printed *to stdout* for the use of tests

    finished.await?;
    Ok(())
}
