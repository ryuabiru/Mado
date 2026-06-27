use std::time::{Duration, Instant};

use mado::nvim::process::{NvimEvent, NvimLaunchOptions, NvimProcess, discover_nvim};
use mado::nvim::redraw::{RedrawEvent, decode_redraw_notification};

#[test]
fn attaches_to_real_neovim_and_receives_flush() {
    let Ok(executable) = discover_nvim() else {
        eprintln!("skipping integration test: Neovim is not installed");
        return;
    };
    let nvim = NvimProcess::spawn(NvimLaunchOptions {
        executable,
        files: vec![],
        clean: true,
    })
    .expect("failed to start Neovim");

    nvim.rpc()
        .request("nvim_get_api_info", vec![])
        .expect("failed to query Neovim API info");
    nvim.attach_ui(80, 24).expect("failed to attach UI");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received_flush = false;
    while Instant::now() < deadline && !received_flush {
        match nvim.events().recv_timeout(Duration::from_millis(250)) {
            Ok(NvimEvent::Notification { method, params }) if method == "redraw" => {
                let events = decode_redraw_notification(&params).expect("invalid redraw batch");
                received_flush = events.contains(&RedrawEvent::Flush);
            }
            Ok(NvimEvent::ProtocolError(error)) => panic!("RPC protocol error: {error}"),
            Ok(NvimEvent::Eof) => panic!("Neovim exited before the first flush"),
            Ok(NvimEvent::Notification { .. }) | Err(_) => {}
        }
    }

    assert!(
        received_flush,
        "Neovim did not send flush within five seconds"
    );
}
