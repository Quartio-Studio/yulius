use std::path::{Path, PathBuf};

use super::fake::{FakeHost, OLD_HOST, OLD_URL, old_host};
use crate::instance::migrate::profile::*;

fn read(host: &mut FakeHost) -> Result<Profile, String> {
    Profile::read(
        host,
        Path::new("/var/lib/yulius"),
        "yulius.service",
        "yulius-update.service",
    )
}

fn paths(list: &[PathBuf]) -> Vec<&str> {
    list.iter().filter_map(|p| p.to_str()).collect()
}

/// SPEC-458 beside SPEC-460 (review of the merge, low): limits for all sandboxes are set on the
/// sessions unit. Its unit file is written by the new host's setup; what an operator added to
/// it travels with the server unit's own drop-ins.
#[test]
fn the_sessions_units_drop_ins_travel_too() -> Result<(), String> {
    let mut host = old_host();
    host.says(
        &["systemctl", "show", "yulius-sandboxes.service"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-sandboxes.service\n\
         DropInPaths=/etc/systemd/system/yulius-sandboxes.service.d/50-limits.conf \
         /etc/systemd/system.control/yulius-sandboxes.service.d/50-TasksMax.conf\n",
    );
    host.file(
        "/etc/systemd/system/yulius-sandboxes.service.d/50-limits.conf",
        "[Service]\nTasksMax=60000\n",
    );
    let p = read(&mut host)?;
    let units: Vec<&str> = p.units.iter().filter_map(|f| f.path.to_str()).collect();
    assert_eq!(
        units.last(),
        Some(&"/etc/systemd/system/yulius-sandboxes.service.d/50-limits.conf"),
        "{units:?}"
    );
    // Its own file is the new host's setup's to write, and systemd's runtime settings are not
    // an operator's file.
    assert!(
        !units
            .iter()
            .any(|path| path.ends_with("yulius-sandboxes.service")
                || path.contains("system.control")),
        "{units:?}"
    );
    Ok(())
}

#[test]
fn the_old_hosts_profile_names_everything_a_move_carries() -> Result<(), String> {
    let mut host = old_host();
    let p = read(&mut host)?;
    assert_eq!(p.release.name(), "ubuntu 24.04");
    assert_eq!(p.release.codename, "noble");
    assert_eq!(
        (p.arch.as_str(), p.coreutils.as_str(), p.cpus),
        ("x86_64", "GNU", 12)
    );
    assert_eq!(p.memory, 49_283_072 * 1024);
    assert_eq!(
        (p.user.as_str(), p.exe.to_str()),
        ("yulius", Some("/usr/local/bin/yulius"))
    );
    assert_eq!(p.public_url.as_deref(), Some(OLD_URL));
    assert_eq!(p.ports, [8080, 8081]);
    assert_eq!(p.used_bytes, 4096 * (94_482_422 - 11_639_808));
    assert_eq!(p.used_inodes, 6_400_000);
    // The unit and the operator's drop-ins; the distribution's drop-in stays.
    let units: Vec<&str> = p.units.iter().filter_map(|f| f.path.to_str()).collect();
    assert_eq!(
        units,
        [
            "/etc/systemd/system/yulius.service",
            "/etc/systemd/system/yulius.service.d/20-limits.conf",
            "/etc/systemd/system/yulius.service.d/30-keys.conf",
        ]
    );
    assert_eq!(
        p.pins,
        ["YULIUS_MAX_SESSIONS=8", "YULIUS_SANDBOX_DISK_GB=300"]
    );
    // Third-party sources only, with the key files they name.
    let sources: Vec<&str> = p.sources.iter().filter_map(|f| f.path.to_str()).collect();
    assert_eq!(
        sources,
        [
            "/etc/apt/sources.list.d/docker.list",
            "/etc/apt/sources.list.d/github-cli.sources",
        ]
    );
    assert_eq!(
        paths(&p.keys),
        [
            "/etc/apt/keyrings/docker.asc",
            "/usr/share/keyrings/githubcli-archive-keyring.gpg",
        ]
    );
    // Carried as they are: a key file outside the data directory (the one inside travels with
    // the data), the environment file, the helper's units and record, Caddy, ufw.
    assert_eq!(
        paths(&p.files),
        [
            "/etc/caddy",
            "/etc/default/ufw",
            "/etc/systemd/system/caddy.service.d",
            "/etc/systemd/system/yulius-update.path",
            "/etc/systemd/system/yulius-update.service",
            "/etc/ufw/ufw.conf",
            "/etc/ufw/user.rules",
            "/etc/ufw/user6.rules",
            "/etc/yulius/env",
            "/etc/yulius/jev.key",
            "/var/lib/yulius-update/installed-commit",
        ]
    );
    assert!(p.helper);
    assert_eq!((&p.proxy, &p.firewall), (&Carried::Yes, &Carried::Yes));
    assert!(p.caddyfile.is_some());
    // A package's directory in /opt comes back with the package.
    let trees: Vec<&str> = p.trees.iter().filter_map(|t| t.path.to_str()).collect();
    assert_eq!(
        trees,
        [
            "/usr/local",
            "/opt/openvscode-server",
            "/root/.rustup",
            "/root/.cargo"
        ]
    );
    assert_eq!(p.trees[0].bytes, 1_048_576);
    // npm's packages are inside /usr/local: copied with it, not installed again.
    assert!(p.npm.is_empty());
    assert_eq!(p.packages.len(), 8);
    // No carried file names the old host.
    assert!(p.stale.is_empty());
    // Known on the new host only once their sources are there.
    assert_eq!(p.third_party, ["docker-ce", "gh"]);
    Ok(())
}

#[test]
fn no_secret_reaches_the_profile() -> Result<(), String> {
    let mut host = old_host();
    let json = serde_json::to_string(&read(&mut host)?).map_err(|e| e.to_string())?;
    for secret in ["jev-secret-value", "token-value", "proxy-secret-value"] {
        assert!(!json.contains(secret), "{secret} is in the profile");
    }
    Ok(())
}

#[test]
fn an_unknown_or_packaged_unit_is_refused() {
    let mut host = old_host();
    host.says(
        &["systemctl", "show", "yulius.service"],
        "LoadState=not-found\n",
    );
    let why = read(&mut host).err().unwrap_or_default();
    assert!(why.contains("there is no unit yulius.service"), "{why}");

    let mut host = old_host();
    host.says(
        &["systemctl", "show", "yulius.service"],
        "LoadState=loaded\nFragmentPath=/usr/lib/systemd/system/yulius.service\nUser=yulius\n",
    );
    let why = read(&mut host).err().unwrap_or_default();
    assert!(why.contains("not under /etc/systemd/system"), "{why}");
}

#[test]
fn a_host_without_helper_proxy_or_firewall_says_so() -> Result<(), String> {
    let mut host = old_host();
    host.says(
        &["systemctl", "show", "yulius-update.service"],
        "LoadState=not-found\n",
    );
    host.says(&["systemctl", "is-active", "caddy"], "inactive\n");
    host.says(&["systemctl", "is-active", "nginx"], "active\n");
    host.file("/etc/ufw/ufw.conf", "ENABLED=no\n");
    host.says(&["systemctl", "is-enabled", "nftables"], "enabled\n");
    let p = read(&mut host)?;
    assert!(!p.helper);
    assert_eq!(p.proxy, Carried::Other("nginx".into()));
    assert_eq!(p.firewall, Carried::Other("nftables".into()));
    assert!(
        !paths(&p.files)
            .iter()
            .any(|f| f.contains("caddy") || f.contains("ufw"))
    );
    Ok(())
}

#[test]
fn npm_packages_outside_the_copied_trees_are_listed_with_their_versions() -> Result<(), String> {
    let mut host = old_host();
    host.says(&["npm", "prefix", "-g"], "/usr\n");
    host.on(
        &["npm", "ls", "-g"],
        1,
        r#"{"dependencies":{"npm":{"version":"10.9.0"},"@anthropic-ai/claude-code":{"version":"2.1.311"},"@openai/codex":{"version":"0.98.0"},"bad name; rm":{"version":"1"}}}"#,
        None,
    );
    let p = read(&mut host)?;
    assert_eq!(
        p.npm,
        ["@anthropic-ai/claude-code@2.1.311", "@openai/codex@0.98.0"]
    );
    Ok(())
}

#[test]
fn a_codename_is_replaced_as_a_word() {
    let list = "deb [signed-by=/k] https://download.docker.com/linux/ubuntu noble stable\n";
    assert_eq!(
        with_codename(list, "noble", "resolute"),
        "deb [signed-by=/k] https://download.docker.com/linux/ubuntu resolute stable\n"
    );
    assert_eq!(
        with_codename("Suites: noble noble-updates\n", "noble", "resolute"),
        "Suites: resolute resolute-updates\n"
    );
    // Part of a longer word, or nothing to do.
    assert_eq!(
        with_codename("https://x/nobleman", "noble", "resolute"),
        "https://x/nobleman"
    );
    assert_eq!(
        with_codename("Suites: stable", "noble", "resolute"),
        "Suites: stable"
    );
    assert_eq!(with_codename("noble", "", "resolute"), "noble");
}

#[test]
fn a_host_name_is_replaced_also_as_the_end_of_a_longer_name() {
    let new = "185-189-47-160.sslip.io";
    let unit = format!("Environment=YULIUS_PUBLIC_URL={OLD_URL}\n");
    assert_eq!(
        with_host(&unit, OLD_HOST, new),
        "Environment=YULIUS_PUBLIC_URL=https://185-189-47-160.sslip.io\n"
    );
    let caddy = format!("{OLD_HOST}, *.{OLD_HOST} {{\n}}\n");
    assert_eq!(
        with_host(&caddy, OLD_HOST, new),
        format!("{new}, *.{new} {{\n}}\n")
    );
    // Another host that only contains the name.
    for other in [
        format!("x{OLD_HOST}"),
        format!("a-{OLD_HOST}"),
        format!("{OLD_HOST}.example.com"),
        format!("{OLD_HOST}-b"),
    ] {
        assert_eq!(with_host(&other, OLD_HOST, new), other);
    }
    // A name that ends a sentence.
    assert_eq!(
        with_host(&format!("at {OLD_HOST}."), OLD_HOST, new),
        format!("at {new}.")
    );
    // The new name holding the old one is not replaced twice.
    assert_eq!(
        with_host("y.example.com", "y.example.com", "new.y.example.com"),
        "new.y.example.com"
    );
}

#[test]
fn an_address_url_follows_the_address_and_a_name_stays() {
    assert_eq!(
        address_url(OLD_URL, "185.189.47.160").as_deref(),
        Some("https://185-189-47-160.sslip.io")
    );
    assert_eq!(
        address_url("https://yulius-1-2-3-4.sslip.io/", "5.6.7.8").as_deref(),
        Some("https://yulius-5-6-7-8.sslip.io/")
    );
    assert_eq!(
        address_url("https://app.1.2.3.4.nip.io", "5.6.7.8").as_deref(),
        Some("https://app.5.6.7.8.nip.io")
    );
    assert_eq!(address_url("https://yulius.example.com", "5.6.7.8"), None);
    assert_eq!(address_url("https://sslip.io", "5.6.7.8"), None);
    // Not an IPv4 address to go to.
    assert_eq!(address_url(OLD_URL, "new-host.example.com"), None);
    assert_eq!(address_url(OLD_URL, "2001:db8::1"), None);
    assert_eq!(
        host_of("https://Yulius.Example.com:8443/x"),
        Some("yulius.example.com".into())
    );
}

#[test]
fn a_library_is_found_under_its_next_version_suffix() {
    let available: Vec<String> = [
        "libicu76",
        "libicu78",
        "libicu-dev",
        "libicuxx1",
        "libssl3t64",
    ]
    .map(String::from)
    .to_vec();
    assert_eq!(stem("libicu74"), Some("libicu"));
    assert_eq!(stem("libssl3t64"), Some("libssl"));
    assert_eq!(stem("python3.12"), Some("python"));
    assert_eq!(stem("docker-ce"), None);
    assert_eq!(stem("x11"), None);
    assert_eq!(renamed("libicu74", &available).as_deref(), Some("libicu78"));
    assert_eq!(
        renamed("libssl3", &available).as_deref(),
        Some("libssl3t64")
    );
    assert_eq!(renamed("libfoo1", &available), None);
    assert_eq!(renamed("docker-ce", &available), None);
    assert!(host_package("linux-image-6.8.0-87-generic") && host_package("cloud-init"));
    assert!(!host_package("libicu74") && !host_package("caddy"));
}

#[test]
fn key_files_are_found_in_both_source_formats() {
    assert_eq!(
        paths(&key_paths(
            "deb [arch=amd64 signed-by=/etc/apt/keyrings/a.asc,/etc/apt/keyrings/b.gpg] u s c\n"
        )),
        ["/etc/apt/keyrings/a.asc", "/etc/apt/keyrings/b.gpg"]
    );
    assert_eq!(
        paths(&key_paths(
            "Types: deb\nSigned-By: /usr/share/keyrings/x.gpg\n"
        )),
        ["/usr/share/keyrings/x.gpg"]
    );
    // An inline key names no file.
    assert!(key_paths("Signed-By:\n -----BEGIN PGP PUBLIC KEY BLOCK-----\n").is_empty());
}

#[test]
fn only_what_a_site_proxied_to_yulius_becomes_the_redirect() {
    let caddyfile = "{\n\temail ops@example.com\n}\n\nyulius.example.com {\n\ttls internal\n\
                     \tencode zstd gzip\n\treverse_proxy 127.0.0.1:8080 {\n\t\theader_up \
                     X-Yulius-Proxy-Secret {env.YULIUS_PROXY_SECRET}\n\t}\n}\n\n\
                     other.example.com {\n\treverse_proxy 127.0.0.1:9000\n}\n";
    let moved = redirected(caddyfile, 8080, "https://new.example.com/").unwrap_or_default();
    // The global options, the site's name and TLS settings and the other site stay.
    assert!(
        moved
            .starts_with("{\n\temail ops@example.com\n}\n\nyulius.example.com {\n\ttls internal\n")
    );
    assert!(
        moved.ends_with("other.example.com {\n\treverse_proxy 127.0.0.1:9000\n}\n"),
        "{moved}"
    );
    assert!(moved.contains("\theader X-Yulius-Moved-To https://new.example.com\n"));
    assert!(
        moved.contains("\tredir https://new.example.com{uri} 307\n}\n"),
        "{moved}"
    );
    // The proxy directive went with its own block.
    assert!(!moved.contains("127.0.0.1:8080") && !moved.contains("header_up"));
    assert_eq!(moved.matches('{').count(), moved.matches('}').count());
    // A one-line directive, and a file that proxies nothing there.
    let one = redirected(
        "a.example {\n  reverse_proxy localhost:8080\n}\n",
        8080,
        "https://n",
    );
    assert_eq!(
        one.as_deref()
            .map(|t| t.contains("  redir https://n{uri} 307\n}\n")),
        Some(true)
    );
    assert_eq!(
        redirected("a.example {\n  respond \"hi\"\n}\n", 8080, "https://n"),
        None
    );
    assert_eq!(redirected(caddyfile, 80, "https://n"), None);
}

#[test]
fn a_rewritten_file_keeps_its_owner() -> Result<(), String> {
    let mut host = FakeHost::default();
    host.file_as("/etc/caddy/Caddyfile", "old", 0, 0o640);
    host.on(&["chown"], 0, "", None);
    // root:root: nothing to give back.
    write_keeping_owner(&mut host, Path::new("/etc/caddy/Caddyfile"), "new", 0o640)?;
    assert!(!host.ran("chown"));
    // Another owner: given back after the write.
    host.file_as("/etc/yulius/unit.conf", "old", 996, 0o600);
    write_keeping_owner(&mut host, Path::new("/etc/yulius/unit.conf"), "new", 0o600)?;
    assert!(
        host.ran("chown -- 996:0 /etc/yulius/unit.conf"),
        "{:?}",
        host.log
    );
    assert_eq!(host.text("/etc/yulius/unit.conf"), Some("new"));
    Ok(())
}

#[test]
fn a_carried_file_that_names_the_old_host_is_named_for_a_person() -> Result<(), String> {
    let mut host = old_host();
    host.file_as(
        "/etc/yulius/env",
        "YULIUS_ALLOWED_ORIGINS=https://109-199-123-240.sslip.io\n",
        0,
        0o600,
    );
    let p = read(&mut host)?;
    assert_eq!(paths(&p.stale), ["/etc/yulius/env"]);
    Ok(())
}

/// SPEC-470 MVB-008: the move helper's units travel when the old host has them, and whether its
/// path unit is enabled there is kept for the new host.
#[test]
fn the_move_helpers_units_travel_with_their_state() -> Result<(), String> {
    let mut host = old_host();
    let p = read(&mut host)?;
    assert_eq!(p.move_helper, None);
    assert!(!paths(&p.files).iter().any(|f| f.contains("migrate")));

    host.says(
        &["systemctl", "show", "yulius-migrate.service"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-migrate.service\n\
         DropInPaths=/etc/systemd/system/yulius-migrate.service.d/10-targets.conf \
         /usr/lib/systemd/system/service.d/10-timeout-abort.conf\n",
    );
    host.says(
        &["systemctl", "show", "yulius-migrate.path"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-migrate.path\nDropInPaths=\n",
    );
    host.says(
        &["systemctl", "is-enabled", "yulius-migrate.path"],
        "enabled\n",
    );
    let p = read(&mut host)?;
    assert_eq!(p.move_helper, Some(true));
    let carried: Vec<&str> = paths(&p.files)
        .into_iter()
        .filter(|f| f.contains("migrate"))
        .collect();
    // An operator's files only: not what the distribution ships for every service.
    assert_eq!(
        carried,
        [
            "/etc/systemd/system/yulius-migrate.path",
            "/etc/systemd/system/yulius-migrate.service",
            "/etc/systemd/system/yulius-migrate.service.d/10-targets.conf",
        ]
    );

    // Installed and switched off: carried, and off on the new host too.
    for state in ["disabled\n", "static\n", ""] {
        host.says(&["systemctl", "is-enabled", "yulius-migrate.path"], state);
        assert_eq!(read(&mut host)?.move_helper, Some(false), "{state:?}");
    }

    // The helper belongs to the server's unit, as its sessions unit does.
    let mut other = old_host();
    other.says(
        &["systemctl", "show", "yulius-migrate.service"],
        "LoadState=not-found\n",
    );
    assert_eq!(read(&mut other)?.move_helper, None);
    Ok(())
}
