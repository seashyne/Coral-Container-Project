//! Subcommand dispatch for the `coral` CLI: parses the global flags, builds the shared
//! [`Ctx`] and routes each command to its group module.

mod compose;
mod containers;
mod images;
mod machine;
mod network;
mod nextgen;
mod oci;
mod runtime;
mod system;
mod volume;

use crate::args::{Cli, Commands};
use crate::daemon_client::resolve_images_dir;
use anyhow::Result;
use coral_cgroups::CgroupManager;
use coral_runtime::{ContainerManager, default_state_root};
use std::path::PathBuf;
use std::process::ExitCode;

/// Shared state available to every command handler.
pub(crate) struct Ctx {
    pub state_root: PathBuf,
    pub images_dir: PathBuf,
    pub addr: String,
    pub mgr: ContainerManager,
}

/// Runs a future on a single-threaded Tokio runtime.
pub(crate) fn run_async<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(fut)
}

pub(crate) fn dispatch(cli: Cli) -> Result<ExitCode> {
    let state_root = cli.root.unwrap_or_else(default_state_root);
    let images_dir = resolve_images_dir(cli.images_dir);
    let addr = cli.addr;
    let mgr = ContainerManager::new(state_root.clone(), CgroupManager::detect());
    let ctx = Ctx {
        state_root,
        images_dir,
        addr,
        mgr,
    };

    match cli.command {
        Commands::Catalog(args) => images::catalog(&ctx, args),
        Commands::Pull(args) => images::pull(&ctx, args),
        Commands::Images(args) => images::images(&ctx, args),
        Commands::Rmi(args) => images::rmi(&ctx, args),
        Commands::Build(args) => images::build(&ctx, args),
        Commands::Tag(args) => images::tag(&ctx, args),
        Commands::Save(args) => images::save(&ctx, args),
        Commands::Load(args) => images::load(&ctx, args),
        Commands::Push(args) => images::push(&ctx, args),
        Commands::Login(args) => images::login(&ctx, args),
        Commands::Logout(args) => images::logout(&ctx, args),
        Commands::Compose(args) => compose::compose(&ctx, args),
        Commands::Diff(args) => containers::diff(&ctx, args),
        Commands::Commit(args) => containers::commit(&ctx, args),
        Commands::Inspect(args) => containers::inspect(&ctx, args),
        Commands::Cp(args) => containers::cp(&ctx, args),
        Commands::Ps => containers::ps(&ctx),
        Commands::Logs(args) => containers::logs(&ctx, args),
        Commands::Exec(args) => containers::exec(&ctx, args),
        Commands::Stop(args) => containers::stop(&ctx, args),
        Commands::Pause(args) => containers::pause(&ctx, args),
        Commands::Unpause(args) => containers::unpause(&ctx, args),
        Commands::Rename(args) => containers::rename(&ctx, args),
        Commands::Update(args) => containers::update(&ctx, args),
        Commands::Rm(args) => containers::rm(&ctx, args),
        Commands::Stats(args) => containers::stats(&ctx, args),
        Commands::Top(args) => containers::top(&ctx, args),
        Commands::Wait(args) => containers::wait(&ctx, args),
        Commands::Wasm(args) => nextgen::wasm(&ctx, args),
        Commands::Vm(args) => nextgen::vm(&ctx, args),
        Commands::Machine(args) => machine::machine(&ctx, args),
        Commands::Sandbox(args) => nextgen::sandbox(&ctx, args),
        Commands::Keygen(args) => nextgen::keygen(&ctx, args),
        Commands::Sign(args) => nextgen::sign(&ctx, args),
        Commands::Verify(args) => nextgen::verify(&ctx, args),
        Commands::Sbom(args) => nextgen::sbom(&ctx, args),
        Commands::Scan(args) => nextgen::scan(&ctx, args),
        Commands::Security(args) => nextgen::security(&ctx, args),
        Commands::System(args) => system::system(&ctx, args),
        Commands::Pods(args) => system::pods(&ctx, args),
        Commands::Version => system::version(&ctx),
        Commands::DaemonVersion => system::daemon_version(&ctx),
        Commands::CriStatus(args) => system::cri_status(&ctx, args),
        Commands::Run(args) => runtime::run(&ctx, args),
        Commands::Create(args) => oci::create(&ctx, args),
        Commands::Start(args) => oci::start(&ctx, args),
        Commands::State(args) => oci::state(&ctx, args),
        Commands::List => oci::list(&ctx),
        Commands::Kill(args) => oci::kill(&ctx, args),
        Commands::Delete(args) => oci::delete(&ctx, args),
        Commands::Spec(args) => oci::spec(&ctx, args),
        Commands::CniConfig(args) => oci::cni_config(&ctx, args),
        Commands::Completion(args) => oci::completion(&ctx, args),
        Commands::Volume(args) => volume::volume(&ctx, args),
        Commands::Network(args) => network::network(&ctx, args),
    }
}
