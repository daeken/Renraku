use renraku_vm::{
    error::Result,
    web::{WebHost, WebServer, WebStage},
};
use std::{path::PathBuf, time::Duration};
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let database = PathBuf::from(args.next().unwrap());
    let ready = PathBuf::from(args.next().unwrap());
    let marker = PathBuf::from(args.next().unwrap());
    let phase = args.next().unwrap();
    let budget = args.next().unwrap().parse().unwrap();
    let server = WebServer::bind(
        WebHost::open(database, "Renraku.Guest.Website::Handle")?,
        "127.0.0.1:0".parse().unwrap(),
        budget,
    )?;
    std::fs::write(ready, server.address()?.to_string())?;
    server.serve_one_with_observer(|stage| {
        let selected = match phase.as_str() {
            "accepted" => WebStage::Accepted,
            "paused" => WebStage::Paused,
            _ => WebStage::Completed,
        };
        if stage == selected {
            std::fs::write(&marker, b"ready")?;
            loop {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    })
}
