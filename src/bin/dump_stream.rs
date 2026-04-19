use std::env;
use std::fs::File;
use std::io::Read;

fn main() {
    let path = env::args().nth(1).expect("file");
    let stream_path = env::args().nth(2).expect("stream");
    let file = File::open(path).unwrap();
    let mut compound = cfb::CompoundFile::open(file).unwrap();
    let mut stream = compound.open_stream(&stream_path).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    println!("LEN {}", buf.len());
    let text = String::from_utf8_lossy(&buf);
    println!("{}", &text[..text.len().min(600)]);
}
