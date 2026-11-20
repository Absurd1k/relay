#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod winter;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--tg-worker") {
        std::process::exit(worker(&args[1..]));
    }
    if args.first().is_some_and(|s| s == "--verify-bundle") {
        let m: relay::bundle::Manifest =
            serde_json::from_str(relay::bundle::MANIFEST).expect("manifest");
        assert_eq!(
            relay::bundle::digest(relay::bundle::PAYLOAD),
            m.bundle_sha256
        );
        return;
    }
    #[cfg(windows)]
    native::run();
}
fn worker(args: &[String]) -> i32 {
    let mut config = match tg_ws_proxy_rs::config::Config::try_from_cli_line(&args.join(" ")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if config.cf_domains.is_empty() && !config.default_domains && !config.check {
        config.cf_domains = tg_ws_proxy_rs::default_domains::fallback_domains();
    }
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing_subscriber::filter::LevelFilter::INFO)
        .try_init();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match rt.block_on(tg_ws_proxy_rs::server::run(config, std::future::pending())) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
