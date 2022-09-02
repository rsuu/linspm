use log;
use simple_logger;
use std::{fs, os::unix::fs::FileExt, path::Path, sync::Arc};

use hyper::{client::HttpConnector, Body, Client, HeaderMap, Method, Request, Response};
use hyper_tls;
use rayon::prelude::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};

use futures::future;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:12.0) Gecko/20100101 Firefox/12.0";

#[tokio::main]
async fn main() {
    init();

    let uri = "https://avatars.githubusercontent.com/u/53087849?v=4";

    let https = hyper_tls::HttpsConnector::new();
    let client = Client::builder().build::<_, hyper::Body>(https);
    let request = Request::builder()
        .method(Method::HEAD)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = client.request(request).await.unwrap();
    let headers = response.headers();

    let info = FileInfo::new(headers, uri, "w", 8);

    let mut join = Vec::with_capacity(info.blocks.len());

    println!("{:#?}", info);
    for f in info.blocks.iter() {
        join.push(async {
            f.download(&client, &info).await.unwrap();
            log::info!("DONE: {}", f.id);
        });
    }

    future::join_all(join).await; // run
}

pub fn init() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Off)
        .with_colors(true)
        .without_timestamps()
        .env()
        .init()
        .unwrap();
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    uri: String,
    len: u64,
    suffix: String,
    save_as: String,
    flag_range: bool,
    thread: u8,
    blocks: Vec<Block>,
    blocks_count: u64,
    block_offset: u64,
    block_offset_head: u64,
    has_write: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum FileType {
    // Video
    Mp4,

    // Image
    Jpeg,
    Png,

    // Audio
    Ogg,

    // Other
    Unknow,
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let suffix_ = match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Mp4 => "mp4",
            Self::Ogg => "ogg",
            _ => "",
        };

        write!(f, "{suffix_}")
    }
}

impl FileInfo {
    fn new(headers: &HeaderMap, uri: &str, save_as: &str, thread: u8) -> Self {
        let len = headers
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let block_offset = len / thread as u64;
        let block_offset_head = len % block_offset;
        let blocks_count = (len / block_offset) + 1;
        let file_type: FileType = if let Some(t) = headers.get("content-type") {
            match t.to_str().unwrap() {
                //"video/x-flv" => ".flv",
                "video/mp4" => FileType::Png,
                //"application/x-mpegURL" => ".m3u8",
                //"video/MP2T" => ".ts",
                //"video/3gpp" => ".3gpp",
                //"video/quicktime" => ".mov",
                //"video/x-msvideo" => ".avi",
                //"video/x-ms-wmv" => ".wmv",
                //"audio/x-wav" => ".wav",
                //"audio/x-mp3" => ".mp3",
                //"audio/mp4" => ".mp4",
                "application/ogg" => FileType::Ogg,
                "image/jpeg" => FileType::Jpeg,
                "image/png" => FileType::Png,
                //"image/tiff" => ".tiff",
                //"image/gif" => ".gif",
                //"image/svg+xml" => ".svg",
                _ => FileType::Unknow,
            }
        } else {
            FileType::Unknow
        };

        let mut blocks = Vec::with_capacity(blocks_count as usize);
        let mut id = 0;
        let mut start = 0;
        let mut end = block_offset_head;

        blocks.push(Block {
            id,
            start,
            end,
            is_done: false,
        });

        for f in 0..blocks_count - 1 {
            id += 1;
            start = end + 1;
            end += block_offset;

            blocks.push(Block::new(id, start, end));
        }

        Self {
            block_offset,
            block_offset_head,
            blocks,
            blocks_count,
            has_write: 0,
            len,
            save_as: format!("{save_as}.{file_type}"),
            suffix: file_type.to_string(),
            thread,
            uri: uri.to_string(),
            flag_range: match headers.get("accept-ranges") {
                None => false,
                Some(v) => v.to_str().unwrap().eq("bytes"),
            },
        }
    }
}

//Content-Length
//Content-Type
//Content-MD5

#[derive(Clone, Copy, Debug)]
struct Block {
    id: u64,
    start: u64,
    end: u64,

    is_done: bool,
}

impl Block {
    fn new(id: u64, start: u64, end: u64) -> Self {
        Self {
            id,
            start,
            end,
            is_done: false,
        }
    }

    async fn download(
        &self,
        client: &Client<hyper_tls::HttpsConnector<HttpConnector>, Body>,
        info: &FileInfo,
    ) -> Result<(), ()> {
        let request = Request::builder()
            .method(Method::GET)
            .header("range", format!("bytes={}-{}", self.start, self.end))
            .uri(info.uri.as_str())
            .body(Body::empty())
            .unwrap();
        let response = client.request(request).await.unwrap();
        let bytes = hyper::body::to_bytes(response).await.unwrap();

        write_file(info.save_as.as_str(), &bytes, self.start)
            .await
            .unwrap();

        Ok(())
    }
}

// #[cfg(any(linux))]
// async fn write_file(filepath: &str, bytes: &[u8], offset: u64) -> Result<usize, std::io::Error> {
//     tokio_uring::start(async {
//         let file = OpenOptions::new()
//             .create(true)
//             .write(true)
//             .open("filepath")
//             .await?;
//         let (res, _) = file.write_at(bytes. offset).await;
//         let n = res?;
//         file.close().await?;
//     })
// }

#[cfg(any(unix))]
async fn write_file(filepath: &str, bytes: &[u8], offset: u64) -> Result<usize, std::io::Error> {
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(filepath)?;
    file.write_at(&bytes, offset)
}

mod test {
    #[test]
    async fn write_bytes_to_file_() {
        write_bytes_to_file("w.txt", "aaa".as_bytes(), 1)
            .await
            .unwrap();
    }
}
