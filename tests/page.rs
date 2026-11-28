//! `lsd --render-page`, the renderer behind web page previews: local files and
//! scripts work, and nothing the page asks for leaves the machine.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::Command;

#[test]
fn renders_local_scripts_and_never_touches_the_network() {
    // A space and a `#` in the folder name must not break the page's URL.
    let dir = std::env::temp_dir().join(format!("lsd page #{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let remote = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    // Red until the local script turns it green. Anything fetched from
    // `remote` would land in the listener's backlog.
    std::fs::write(
        dir.join("app.js"),
        "setTimeout(() => { document.body.style.background = 'rgb(0, 200, 0)' }, 20);",
    )
    .unwrap();
    std::fs::write(
        dir.join("index.html"),
        format!(
            r#"<!doctype html>
<link rel="stylesheet" href="{remote}/style.css">
<body style="margin: 0; background: rgb(200, 0, 0)">
<h1>A page</h1>
<img src="{remote}/pixel.png">
<script>
  fetch("{remote}/fetch").catch(() => {{}});
  navigator.sendBeacon("{remote}/beacon", "x");
  try {{ new WebSocket("{ws}/socket") }} catch {{}}
</script>
<script src="app.js"></script>
</body>"#,
            ws = remote.replace("http", "ws"),
        ),
    )
    .unwrap();

    let out = dir.join("page.png");
    let status = Command::new(env!("CARGO_BIN_EXE_lsd"))
        .arg("--render-page")
        .arg(dir.join("index.html"))
        .arg(&out)
        .arg("256")
        .status()
        .unwrap();
    let picture = image::open(&out).map(|img| img.to_rgba8());
    let reached = listener.accept();
    std::fs::remove_dir_all(&dir).unwrap();

    if status.code() == Some(2) {
        eprintln!("skipped: no page renderer on this machine");
        return;
    }
    assert!(status.success(), "{status}");
    let picture = picture.unwrap();
    assert_eq!(picture.height(), 256);
    let [r, g, b, _] = picture
        .get_pixel(picture.width() / 2, picture.height() / 2)
        .0;
    assert!(
        g > 150 && r < 60 && b < 60,
        "script did not run: {r},{g},{b}"
    );
    assert!(
        matches!(&reached, Err(e) if e.kind() == ErrorKind::WouldBlock),
        "the page reached the network: {reached:?}"
    );
}

#[test]
fn a_page_that_draws_nothing_is_not_a_picture() {
    // Its app comes from the web, which is blocked, so it stays blank.
    let dir = std::env::temp_dir().join(format!("lsd blank page {}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        r#"<!doctype html><div id="root"></div><script src="https://example.com/app.js"></script>"#,
    )
    .unwrap();
    let out = dir.join("page.png");
    let status = Command::new(env!("CARGO_BIN_EXE_lsd"))
        .arg("--render-page")
        .arg(dir.join("index.html"))
        .arg(&out)
        .arg("256")
        .status()
        .unwrap();
    let written = out.exists();
    std::fs::remove_dir_all(&dir).unwrap();
    if status.code() == Some(2) {
        eprintln!("skipped: no page renderer on this machine");
        return;
    }
    assert_eq!(status.code(), Some(1));
    assert!(!written);
}
