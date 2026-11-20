use std::time::Duration;

#[test]
fn rust_proxy_binds_on_loopback_and_releases_port_on_shutdown() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let addr = rt.block_on(async {
        let config = tg_ws_proxy_rs::config::Config::try_from_cli_line(
            "--host 127.0.0.1 --port 0 --secret 00112233445566778899aabbccddeeff --pool-size 0 --max-connections 8 --no-outbound-proxy",
        ).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (ready, bound) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(tg_ws_proxy_rs::server::run_with_listen(
            config,
            async { let _ = stopped.await; },
            move |info| { let _ = ready.send(info.addr); },
        ));
        let addr = tokio::time::timeout(Duration::from_secs(5), bound).await.unwrap().unwrap();
        assert!(addr.ip().is_loopback());
        let socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        drop(socket);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
        addr
    });
    drop(rt);
    assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
}

#[cfg(windows)]
#[test]
fn windows_job_stops_the_real_embedded_worker() {
    use std::process::{Command, Stdio};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let job = relay::platform::Job::new().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_relay"));
    cmd.args([
        "--tg-worker",
        "--host",
        "127.0.0.1",
        "--port",
        &addr.port().to_string(),
        "--secret",
        "00112233445566778899aabbccddeeff",
        "--pool-size",
        "0",
        "--no-outbound-proxy",
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .stdin(Stdio::null());
    relay::platform::hidden(&mut cmd);
    let mut child = cmd.spawn().unwrap();
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("{e}");
    }
    let mut bound = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(job);
    let mut exited = false;
    for _ in 0..50 {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(bound, "embedded worker did not bind");
    assert!(exited, "worker survived closing its owning Windows Job");
}
