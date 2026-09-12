// Synthetic protocol peer compiled by the helper lifecycle tests. No native OCR.
use std::io::{Read, Write};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    std::fs::write(&args[2], std::process::id().to_string()).unwrap();
    if args[1] == "hang" {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    if args[1] == "crash" {
        std::process::exit(42);
    }
    let mut input = std::io::stdin();
    let mut len = [0; 4];
    input.read_exact(&mut len).unwrap();
    let mut header = vec![0; u32::from_be_bytes(len) as usize];
    input.read_exact(&mut header).unwrap();
    let header = String::from_utf8(header).unwrap();
    let id = header
        .split("\"request_id\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let image_len: usize = header
        .split("\"image_len\":")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut image = vec![0; image_len];
    input.read_exact(&mut image).unwrap();
    let id = if args[1] == "wrong_id" {
        "00000000-0000-0000-0000-000000000000"
    } else {
        id
    };
    let response = if args[1] == "bad_json" {
        "not json".into()
    } else if args[1] == "native_error" {
        format!("{{\"request_id\":\"{id}\",\"result\":null,\"error_kind\":\"failed\",\"error_message\":\"synthetic native failure\"}}")
    } else {
        format!("{{\"request_id\":\"{id}\",\"result\":{{\"text\":\"fixture\",\"confidence\":1.0,\"languages\":[],\"mode\":\"test\",\"boxes\":[]}},\"error_kind\":null,\"error_message\":null}}")
    };
    let mut output = std::io::stdout();
    if args[1] == "bad_length" {
        output.write_all(&u32::MAX.to_be_bytes()).unwrap();
    } else {
        output
            .write_all(&(response.len() as u32).to_be_bytes())
            .unwrap();
        if args[1] != "truncated" {
            output.write_all(response.as_bytes()).unwrap();
        }
    }
    output.flush().unwrap();
    if args[1] == "exit_hang" {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
