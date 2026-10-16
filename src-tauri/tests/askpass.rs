//! Exercise the actual GUI executable's stdout-only helper branch without a user
//! credential, a desktop, or any connection to an SSH server.
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn actual_executable_returns_only_the_one_time_fixture() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let expected_token = token.clone();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "密码辅助进程未连接测试端口");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut provided = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut provided)
            .unwrap();
        assert_eq!(provided.trim_end(), expected_token);
        stream
            .write_all(b"fixture-only-not-a-user-password\n")
            .unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_termius-image-paste"))
        .arg("fixture@localhost's password: ")
        .env("TERMIUS_ASKPASS_ADDR", address.to_string())
        .env("TERMIUS_ASKPASS_TOKEN", token)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"fixture-only-not-a-user-password\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn otp_prompt_never_requests_a_password() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_termius-image-paste"))
        .arg("Verification code: ")
        .env(
            "TERMIUS_ASKPASS_ADDR",
            listener.local_addr().unwrap().to_string(),
        )
        .env(
            "TERMIUS_ASKPASS_TOKEN",
            uuid::Uuid::new_v4().simple().to_string(),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(matches!(listener.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock));
}
