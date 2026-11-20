//! Entry point and subcommand dispatcher for the `coral` CLI.

mod args;
mod catalog;
mod cp;
mod daemon_client;
mod local_run;
mod nextgen;
mod port_proxy;

use anyhow::{Context, Result};
use args::{
    Cli, Commands, ComposeAction, NetworkAction, PodAction, SandboxAction, SystemAction, VmAction,
    VmDiskAction,
    VolumeAction, WasmAction,
};
use clap::{CommandFactory, Parser};
use coral_cgroups::CgroupManager;
use coral_daemon::images;
use coral_net::CniNetworkConfig;
use coral_oci::OciSpec;
use coral_runtime::container::parse_signal;
use coral_runtime::overlay::diff_overlay;
use coral_runtime::rootfs::{
    default_volumes_dir, ensure_named_volume, list_named_volumes, remove_named_volume,
};
use coral_runtime::{ContainerManager, ExecOptions, RunOptions, StateStore, default_state_root};
use daemon_client::{DaemonRunOptions, resolve_images_dir};
use local_run::{
    LocalImageRunArgs, auto_container_id, commit_local_container, resolve_local_container_overlay,
    run_from_image_local,
};
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("coral error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_async<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(fut)
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    let state_root = cli.root.unwrap_or_else(default_state_root);
    let images_dir = resolve_images_dir(cli.images_dir);
    let addr = cli.addr;
    let mgr = ContainerManager::new(state_root.clone(), CgroupManager::detect());

    match cli.command {
        Commands::Catalog { local } => {
            run_async(daemon_client::cmd_catalog(&addr, &images_dir, local))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Pull {
            image,
            rootfs,
            local,
        } => {
            run_async(daemon_client::cmd_pull(
                &addr,
                &images_dir,
                &image,
                rootfs.as_deref(),
                local,
            ))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Images { local } => {
            run_async(daemon_client::cmd_images(&addr, &images_dir, local))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Rmi { image, local } => {
            run_async(daemon_client::cmd_rmi(&addr, &images_dir, &image, local))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Build {
            tag,
            file,
            context,
            local,
            no_cache,
        } => {
            let dockerfile_path = file.unwrap_or_else(|| context.join("Dockerfile"));
            run_async(daemon_client::cmd_build(
                &addr,
                &state_root,
                &images_dir,
                &tag,
                &dockerfile_path,
                &context,
                local,
                no_cache,
            ))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Tag {
            source,
            target,
            local,
        } => {
            run_async(daemon_client::cmd_tag(
                &addr,
                &images_dir,
                &source,
                &target,
                local,
            ))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Save {
            output,
            image,
            local,
        } => {
            run_async(daemon_client::cmd_save(
                &addr,
                &images_dir,
                &image,
                &output,
                local,
            ))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Load {
            input,
            tag,
            local,
        } => {
            run_async(daemon_client::cmd_load(
                &addr,
                &images_dir,
                &input,
                tag.as_deref(),
                local,
            ))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Compose {
            file,
            project_name,
            action,
        } => {
            match action {
                ComposeAction::Up { detach, build } => {
                    run_async(daemon_client::cmd_compose_up(
                        &addr,
                        &state_root,
                        &images_dir,
                        file.as_deref(),
                        project_name.as_deref(),
                        detach,
                        build,
                    ))?;
                }
                ComposeAction::Down { volumes } => {
                    run_async(daemon_client::cmd_compose_down(
                        &addr,
                        file.as_deref(),
                        project_name.as_deref(),
                        volumes,
                    ))?;
                }
                ComposeAction::Ps => {
                    run_async(daemon_client::cmd_compose_ps(
                        &addr,
                        file.as_deref(),
                        project_name.as_deref(),
                    ))?;
                }
                ComposeAction::Logs { service } => {
                    run_async(daemon_client::cmd_compose_logs(
                        &addr,
                        file.as_deref(),
                        project_name.as_deref(),
                        service.as_deref(),
                    ))?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Diff { id, local } => {
            if !local
                && let Ok(Some(diff_txt)) = run_async(daemon_client::cmd_diff_via_daemon(&addr, &id))
            {
                if diff_txt.trim().is_empty() {
                    println!("(no OverlayFS changes recorded for '{id}')");
                } else {
                    println!("{diff_txt}");
                }
                return Ok(ExitCode::SUCCESS);
            }

            let (lower_rootfs, upperdir) = resolve_local_container_overlay(&state_root, &id)?;
            let changes = diff_overlay(&upperdir, Some(&lower_rootfs))
                .with_context(|| format!("inspect OverlayFS diff for '{id}'"))?;
            if changes.is_empty() {
                println!("(no OverlayFS changes recorded for '{id}')");
            } else {
                for ch in changes {
                    println!("{ch}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Commit {
            container,
            image,
            local,
        } => {
            if !local
                && let Ok(Some(image_ref)) = run_async(daemon_client::cmd_commit_via_daemon(
                    &addr,
                    &container,
                    &image,
                ))
            {
                println!("Committed {container} -> {image} ({image_ref})");
                return Ok(ExitCode::SUCCESS);
            }

            commit_local_container(&state_root, &images_dir, &container, &image)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Volume { action } => {
            let vol_root = default_volumes_dir();
            match action.unwrap_or(VolumeAction::Ls) {
                VolumeAction::Create { name } => {
                    let _mountpoint = ensure_named_volume(&vol_root, &name)
                        .with_context(|| format!("create volume '{name}'"))?;
                    println!("{name}");
                }
                VolumeAction::Ls => {
                    let vols = list_named_volumes(&vol_root)
                        .with_context(|| format!("list volumes in {}", vol_root.display()))?;
                    if vols.is_empty() {
                        println!("No named volumes in {}.", vol_root.display());
                    } else {
                        println!("{:<24} {:>10}  MOUNTPOINT", "VOLUME NAME", "SIZE");
                        for (vname, mountpoint) in vols {
                            let size = images::dir_size(&mountpoint);
                            println!(
                                "{:<24} {:>10}  {}",
                                vname,
                                catalog::format_bytes(size),
                                mountpoint.display()
                            );
                        }
                    }
                }
                VolumeAction::Inspect { name } => {
                    let vols = list_named_volumes(&vol_root)?;
                    let (vname, mountpoint) = vols
                        .into_iter()
                        .find(|(vn, _)| vn == &name)
                        .ok_or_else(|| anyhow::anyhow!("volume '{name}' not found"))?;
                    let size = images::dir_size(&mountpoint);
                    let info = serde_json::json!({
                        "name": vname,
                        "mountpoint": mountpoint,
                        "size": size,
                    });
                    println!("{}", serde_json::to_string_pretty(&info)?);
                }
                VolumeAction::Rm { name } => {
                    remove_named_volume(&vol_root, &name)
                        .with_context(|| format!("remove volume '{name}'"))?;
                    println!("Removed volume '{name}'");
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Inspect { id, local } => {
            if !local
                && let Ok(Some(json)) = run_async(daemon_client::cmd_inspect_via_daemon(&addr, &id))
            {
                println!("{json}");
                return Ok(ExitCode::SUCCESS);
            }
            let st = StateStore::new(&state_root)
                .load(&id)
                .or_else(|_| StateStore::new(state_root.join("daemon/runtime-state")).load(&id))
                .with_context(|| format!("inspect container '{id}'"))?;
            let spec = OciSpec::from_file(st.bundle.join("config.json")).ok();
            let info = serde_json::json!({
                "state": st,
                "spec": spec,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
            Ok(ExitCode::SUCCESS)
        }

        Commands::Cp { src, dst } => {
            run_async(cp::cmd_cp(&addr, &state_root, &src, &dst))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Login {
            registry,
            username,
            password,
            password_stdin,
        } => {
            let pass = if password_stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf.trim_end_matches(&['\r', '\n'][..]).to_string()
            } else if let Some(p) = password {
                p
            } else {
                anyhow::bail!("provide --password <TOKEN> or --password-stdin");
            };
            run_async(daemon_client::cmd_login(&addr, &registry, &username, &pass))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Logout { registry } => {
            run_async(daemon_client::cmd_logout(&addr, &registry))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Push { image, local } => {
            run_async(daemon_client::cmd_push(&addr, &images_dir, &image, local))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Network { action } => {
            match action.unwrap_or(NetworkAction::Ls) {
                NetworkAction::Create {
                    name,
                    driver,
                    subnet,
                } => {
                    let net = coral_net::create_network(&name, Some(&driver), subnet.as_deref())?;
                    println!(
                        "Created network '{}' (driver={}, subnet={}, gateway={})",
                        net.name, net.driver, net.subnet, net.gateway
                    );
                }
                NetworkAction::Ls => {
                    let nets = coral_net::list_networks();
                    let egress = coral_net::detect_egress_backend()
                        .map(|(b, _)| b.binary_name())
                        .unwrap_or("none (install pasta or slirp4netns for rootless egress)");
                    println!(
                        "{:<20} {:<10} {:<18} {:<16} SCOPE",
                        "NAME", "DRIVER", "SUBNET", "GATEWAY"
                    );
                    for n in nets {
                        println!(
                            "{:<20} {:<10} {:<18} {:<16} {}",
                            n.name,
                            n.driver,
                            if n.subnet.is_empty() { "-" } else { &n.subnet },
                            if n.gateway.is_empty() { "-" } else { &n.gateway },
                            if n.builtin { "built-in" } else { "local" }
                        );
                    }
                    println!("Rootless bridge egress backend: {egress}");
                }
                NetworkAction::Inspect { name } => {
                    let net = coral_net::find_network(&name)
                        .ok_or_else(|| anyhow::anyhow!("network '{name}' not found"))?;
                    println!("{}", serde_json::to_string_pretty(&net)?);
                }
                NetworkAction::Rm { name } => {
                    coral_net::remove_network(&name)?;
                    println!("Removed network '{name}'");
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Wasm { action } => match action {
            WasmAction::Run {
                target,
                fuel,
                memory_mb,
                envs,
                json,
                args,
            } => nextgen::cmd_wasm_run(&images_dir, &target, fuel, memory_mb, envs, args, json),
            WasmAction::Pack { file, tag } => {
                nextgen::cmd_wasm_pack(&images_dir, &file, &tag)?;
                Ok(ExitCode::SUCCESS)
            }
            WasmAction::Serve {
                target,
                listen,
                fuel,
                memory_mb,
                envs,
            } => {
                run_async(nextgen::cmd_wasm_serve(
                    &images_dir,
                    &target,
                    &listen,
                    fuel,
                    memory_mb,
                    envs,
                ))?;
                Ok(ExitCode::SUCCESS)
            }
        },

        Commands::Vm { action } => match action {
            VmAction::Probe { json } => {
                nextgen::cmd_vm_probe(json)?;
                Ok(ExitCode::SUCCESS)
            }
            VmAction::SetupKernel { from, dest, json } => {
                nextgen::cmd_vm_setup_kernel(from, dest, json)?;
                Ok(ExitCode::SUCCESS)
            }
            VmAction::Boot {
                image,
                kernel,
                memory_mb,
                timeout,
                cmdline,
                disk,
                disk_mount,
                net,
                ports,
                json,
                cmd,
            } => nextgen::cmd_vm_boot(
                &state_root,
                &images_dir,
                &image,
                kernel,
                memory_mb,
                timeout,
                cmdline,
                disk,
                disk_mount,
                net || !ports.is_empty(),
                ports,
                &cmd,
                json,
            ),
            VmAction::Disk { action } => match action {
                VmDiskAction::Create {
                    path,
                    size_mb,
                    ext4,
                } => {
                    nextgen::cmd_vm_disk_create(&path, size_mb, ext4)?;
                    Ok(ExitCode::SUCCESS)
                }
            },
            VmAction::Run {
                payload,
                kernel,
                initrd,
                cmdline,
                memory_mb,
                timeout,
                max_exits,
                json,
            } => nextgen::cmd_vm_run(
                payload, kernel, initrd, cmdline, memory_mb, timeout, max_exits, json,
            ),
        },

        Commands::Sandbox { action } => match action {
            SandboxAction::Run {
                image,
                timeout,
                memory_mb,
                cpu_percent,
                pids_limit,
                allow_network,
                kernel,
                envs,
                json,
                cmd,
            } => nextgen::cmd_sandbox_run(
                &state_root,
                &images_dir,
                image,
                cmd,
                envs,
                memory_mb,
                cpu_percent,
                pids_limit,
                timeout,
                allow_network,
                kernel,
                json,
            ),
        },

        Commands::System { action } => {
            match action {
                SystemAction::Df => {
                    run_async(daemon_client::cmd_system_df(&addr, &state_root, &images_dir))?;
                }
                SystemAction::Prune { all, volumes } => {
                    run_async(daemon_client::cmd_system_prune(&addr, all, volumes))?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Run {
            bundle,
            image,
            runtime,
            name,
            interactive,
            tty,
            volumes,
            ports,
            network,
            dns,
            extra_hosts,
            no_overlay,
            privileged,
            envs,
            memory_mb,
            cpu_percent,
            pids_limit,
            workdir,
            user,
            entrypoint,
            hostname,
            restart,
            health_cmd,
            health_interval,
            health_retries,
            daemon,
            detach,
            keep,
            rm,
            id,
            mut cmd,
        } => {
            let keep = keep && !rm;
            if let Some(image_ref) = image {
                // Direct Wasm runtime or auto-detected Wasm OCI image!
                if runtime.eq_ignore_ascii_case("wasm")
                    || (!daemon
                        && !detach
                        && runtime.eq_ignore_ascii_case("oci")
                        && nextgen::resolve_wasm_target(&images_dir, &image_ref).is_ok())
                {
                    let mem = if memory_mb > 0 { memory_mb as usize } else { 64 };
                    return nextgen::cmd_wasm_run(
                        &images_dir,
                        &image_ref,
                        50_000_000,
                        mem,
                        envs,
                        cmd,
                        false,
                    );
                }
                // Direct AI Agent Sandbox / MicroVM runtime!
                if runtime.eq_ignore_ascii_case("vmm") || runtime.eq_ignore_ascii_case("sandbox") {
                    return nextgen::cmd_sandbox_run(
                        &state_root,
                        &images_dir,
                        image_ref,
                        cmd,
                        envs,
                        if memory_mb > 0 { memory_mb } else { 64 },
                        if cpu_percent > 0 { cpu_percent } else { 50 },
                        if pids_limit > 0 { pids_limit } else { 32 },
                        30,
                        !network.eq_ignore_ascii_case("none"),
                        None,
                        false,
                    );
                }

                let explicit_id = match (name, id) {
                    (Some(n), Some(pos)) if cmd.is_empty() => {
                        cmd.push(pos);
                        Some(n)
                    }
                    (Some(n), _) => Some(n),
                    (None, Some(pos)) if pos.starts_with('/') && cmd.is_empty() => {
                        cmd.push(pos);
                        None
                    }
                    (None, Some(pos)) => Some(pos),
                    (None, None) => None,
                };
                let container_id = explicit_id.unwrap_or_else(|| auto_container_id(&image_ref));

                if daemon || detach {
                    return run_async(daemon_client::cmd_run_via_daemon(
                        &addr,
                        DaemonRunOptions {
                            name: container_id,
                            image: image_ref,
                            command: cmd,
                            envs,
                            volumes,
                            ports,
                            network,
                            dns,
                            extra_hosts,
                            privileged,
                            no_overlay,
                            workdir,
                            user,
                            entrypoint,
                            hostname,
                            memory_mb,
                            cpu_percent,
                            pids_limit,
                            restart,
                            health_cmd,
                            health_interval_secs: health_interval,
                            health_retries,
                            detach,
                        },
                    ));
                }

                return run_from_image_local(
                    &mgr,
                    &state_root,
                    &images_dir,
                    LocalImageRunArgs {
                        image: image_ref,
                        container_id,
                        cmd,
                        envs,
                        volumes,
                        ports,
                        network,
                        dns,
                        extra_hosts,
                        no_overlay,
                        privileged,
                        tty,
                        interactive,
                        memory_mb,
                        cpu_percent,
                        pids_limit,
                        workdir,
                        user,
                        entrypoint,
                        hostname,
                        keep,
                    },
                );
            }

            // Standard OCI bundle mode: MUST remain single-threaded (zero Tokio runtime) for fork/unshare safety.
            let container_id = name.or(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "container ID is required in OCI bundle mode (e.g. `coral run --bundle . my-box`, or pass `--image <IMAGE>`)"
                )
            })?;
            let exit = mgr
                .run(
                    &container_id,
                    &bundle,
                    &RunOptions {
                        keep_state: keep,
                        tty,
                        interactive,
                    },
                )
                .with_context(|| format!("run container '{container_id}'"))?;
            let code = u8::try_from(exit.exit_code.clamp(0, 255)).unwrap_or(1);
            Ok(ExitCode::from(code))
        }

        Commands::Ps => {
            run_async(daemon_client::cmd_ps(&addr))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Logs {
            id,
            status,
            follow,
            tail,
        } => {
            run_async(daemon_client::cmd_logs(&addr, &id, status, follow, tail))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Exec {
            id,
            local,
            interactive,
            tty,
            envs,
            workdir,
            timeout,
            cmd,
        } => {
            if local || interactive || tty {
                // Single-threaded `setns()` directly into live container's namespaces!
                // Auto-detects containers running in either local state_root or coral-daemon's runtime-state.
                let exec_opts = ExecOptions {
                    cmd,
                    env: envs,
                    cwd: workdir,
                    tty,
                    interactive,
                };
                let target_mgr = if StateStore::new(&state_root).load(&id).is_ok() {
                    mgr
                } else {
                    let daemon_rt = state_root.join("daemon/runtime-state");
                    if StateStore::new(&daemon_rt).load(&id).is_ok() {
                        ContainerManager::new(daemon_rt, CgroupManager::detect())
                    } else {
                        mgr
                    }
                };
                let exit_code = target_mgr
                    .exec(&id, &exec_opts)
                    .with_context(|| format!("exec into container '{id}'"))?;
                let code = u8::try_from(exit_code.clamp(0, 255)).unwrap_or(1);
                return Ok(ExitCode::from(code));
            }
            run_async(daemon_client::cmd_exec(&addr, &id, timeout, cmd))
        }

        Commands::Stop { id, timeout } => {
            run_async(daemon_client::cmd_stop(&addr, &id, timeout))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Rm { id } => {
            run_async(daemon_client::cmd_rm(&addr, &id))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Pods { action } => {
            match action.unwrap_or(PodAction::Ls) {
                PodAction::Ls => run_async(daemon_client::cmd_pods_list(&addr))?,
                PodAction::Stop { id } => run_async(daemon_client::cmd_pods_stop(&addr, &id))?,
                PodAction::Rm { id } => run_async(daemon_client::cmd_pods_rm(&addr, &id))?,
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Version => {
            run_async(daemon_client::cmd_version(&addr, false))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Spec {
            rootless,
            rootfs,
            output,
        } => {
            let host_uid = unsafe { libc::geteuid() };
            let host_gid = unsafe { libc::getegid() };
            let use_rootless = rootless || host_uid != 0;
            let mut spec = OciSpec::default_spec(use_rootless, host_uid, host_gid);
            if let Some(r) = spec.root.as_mut() {
                r.path = rootfs;
            }
            spec.to_file(&output)
                .with_context(|| format!("write OCI spec to {}", output.display()))?;
            println!(
                "Wrote OCI v{} spec to {}",
                spec.oci_version,
                output.display()
            );
            Ok(ExitCode::SUCCESS)
        }

        Commands::Create { bundle, id } => {
            let state = mgr
                .create(&id, &bundle)
                .with_context(|| format!("create container '{id}'"))?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(ExitCode::SUCCESS)
        }

        Commands::Start { id } => {
            let state = mgr
                .start(&id)
                .with_context(|| format!("start container '{id}'"))?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(ExitCode::SUCCESS)
        }

        Commands::State { id } => {
            let state = mgr
                .state(&id)
                .with_context(|| format!("read state for '{id}'"))?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(ExitCode::SUCCESS)
        }

        Commands::List => {
            let list = mgr.list().context("list containers")?;
            println!(
                "{:<20} {:<10} {:<10} {:<26} BUNDLE",
                "ID", "PID", "STATUS", "CREATED"
            );
            for item in list {
                let pid_str = item
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{:<20} {:<10} {:<10} {:<26} {}",
                    item.id,
                    pid_str,
                    item.status,
                    item.created.to_rfc3339(),
                    item.bundle.display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Kill { id, signal } => {
            let sig = parse_signal(&signal)?;
            mgr.kill(&id, sig)
                .with_context(|| format!("kill container '{id}' with {sig}"))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Delete { force, id } => {
            let cli_bundle = mgr.state(&id).ok().map(|s| s.bundle);
            mgr.delete(&id, force)
                .with_context(|| format!("delete container '{id}'"))?;
            if let Some(bundle_path) = cli_bundle
                && bundle_path.starts_with(state_root.join(".cli-bundles"))
            {
                let _ = images::remove_dir_force(&bundle_path);
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Stats {
            id,
            watch,
            interval,
        } => {
            if run_async(daemon_client::cmd_stats(&addr, &id, watch, interval))? {
                return Ok(ExitCode::SUCCESS);
            }
            let stats = mgr
                .cgroups()
                .read_stats(&id)
                .with_context(|| format!("read cgroup stats for '{id}'"))?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
            Ok(ExitCode::SUCCESS)
        }

        Commands::CniConfig { output } => {
            let cfg = CniNetworkConfig::default_bridge();
            if let Some(path) = output {
                cfg.to_file(&path)?;
                println!("Wrote CNI v{} config to {}", cfg.cni_version, path.display());
            } else {
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Keygen {
            key,
            pub_key,
            force,
            json,
        } => {
            nextgen::cmd_keygen(key, pub_key, force, json)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Sign { image, key, json } => {
            nextgen::cmd_sign(&images_dir, &image, key, json)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Verify { image, key, json } => {
            nextgen::cmd_verify(&images_dir, &image, key, json)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Sbom { image, output } => {
            nextgen::cmd_sbom(&images_dir, &image, output)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::CriStatus { verbose } => {
            run_async(daemon_client::cmd_cri_status(&addr, verbose))?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Completion { shell } => {
            use std::io::Write;
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "coral", &mut buf);
            if let Err(e) = std::io::stdout().write_all(&buf)
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(e.into());
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::DaemonVersion => {
            run_async(daemon_client::cmd_version(&addr, true))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
