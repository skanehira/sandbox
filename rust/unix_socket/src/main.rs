#![allow(unused)]

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::iter::{FromIterator, Map};
use std::os::unix::net::UnixStream;

pub trait ReadWriter: io::Read + io::Write {}

// NOTE: io::Read と io::Write を満たしているすべての T に対して、ReadWriter を実装する
// つまり、これで io::Read と io::Write 両方を実装している構造体などに ReadWriter
// を実装したことになる
impl<T> ReadWriter for T where T: io::Read + io::Write {}

pub struct HttpClient<T: ReadWriter> {
    conn: T,
}

pub enum HttpMethod {
    Get,
    Post,
    Update,
    Delete,
    Patch,
}

impl Default for HttpMethod {
    fn default() -> Self {
        Self::Get
    }
}

impl Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let method = match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
        };
        write!(f, "{}", method)
    }
}

#[derive(Debug, Clone)]
pub enum MapKey {
    String(String),
    Number(isize),
}

impl Display for MapKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key = match self {
            Self::String(s) => s.to_owned(),
            Self::Number(n) => n.clone().to_string(),
        };
        write!(f, "{}", key)
    }
}

#[derive(Debug, Clone)]
pub struct HttpHeader(BTreeMap<String, MapKey>);

impl Display for HttpHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut h = Vec::new();
        for (k, v) in self.0.iter() {
            h.push(format!("{}: {}", k, v));
        }
        write!(f, "{}", h.join("\r\n"),)
    }
}

impl HttpHeader {
    fn new() -> Self {
        Self { 0: BTreeMap::new() }
    }
    fn add(&mut self, key: &str, value: MapKey) {
        self.0.insert(key.to_string(), value);
    }
}

impl<'a> FromIterator<(&'a str, MapKey)> for HttpHeader {
    fn from_iter<T: IntoIterator<Item = (&'a str, MapKey)>>(iter: T) -> Self {
        let mut p = Self::new();
        for (k, v) in iter {
            p.add(k, v);
        }
        p
    }
}

#[derive(Debug)]
pub struct HttpParams(BTreeMap<String, MapKey>);

impl Display for HttpParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut buf = Vec::<String>::new();
        for (k, v) in self.0.iter() {
            buf.push(format!("{}={}", k, v));
        }
        write!(f, "{}", buf.join("&"))
    }
}

impl HttpParams {
    fn new() -> Self {
        Self { 0: BTreeMap::new() }
    }
    fn add(&mut self, key: &str, value: MapKey) {
        self.0.insert(key.to_string(), value);
    }
}

impl<'a> FromIterator<(&'a str, MapKey)> for HttpParams {
    fn from_iter<T: IntoIterator<Item = (&'a str, MapKey)>>(iter: T) -> Self {
        let mut p = Self::new();
        for (k, v) in iter {
            p.add(k, v);
        }
        p
    }
}

#[derive(Default)]
pub struct Request {
    url: String,
    base_url: Option<String>,
    method: HttpMethod,
    header: Option<HttpHeader>,
    params: Option<HttpParams>,
    body: Option<Vec<u8>>,
}

impl Request {
    fn new(url: String) -> Self {
        Self {
            url,
            ..Default::default()
        }
    }

    fn base_url(&mut self, p: String) -> &mut Self {
        self.base_url = Some(p);
        self
    }

    fn method(&mut self, p: HttpMethod) -> &mut Self {
        self.method = p;
        self
    }

    fn header(&mut self, p: HttpHeader) -> &mut Self {
        self.header = Some(p);
        self
    }

    fn params(&mut self, p: HttpParams) -> &mut Self {
        self.params = Some(p);
        self
    }

    fn body(&mut self, p: Vec<u8>) -> &mut Self {
        self.body = Some(p);
        self
    }

    fn get(url: String) -> Self {
        let mut request = Self::new(url);
        request.method(HttpMethod::Get);
        request
    }

    fn build(&mut self) -> Vec<u8> {
        let url = match &self.params {
            Some(params) => {
                format!("{}?{}", self.url, params)
            }
            None => self.url.clone(),
        };

        let base_url = match &self.base_url {
            Some(base_url) => base_url.clone(),
            None => "localhost".to_string(),
        };

        let mut body = vec![
            format!("{} {} HTTP/1.1", self.method, url),
            format!("Host: {}", base_url),
        ];
        if let Some(header) = &self.header {
            body.push(format!("{}\r\n", header));
        }

        let mut body = body.join("\r\n").as_bytes().to_vec();
        body.append(&mut "\r\n".as_bytes().to_vec());
        if let Some(data) = &self.body {
            body.append(&mut data.to_vec());
        }
        body.append(&mut "\r\n".as_bytes().to_vec());
        body
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    status: u32,
    header: HttpHeader,
    body: Option<Vec<u8>>,
}

impl<T: ReadWriter> HttpClient<T> {
    fn new(conn: T) -> Self {
        HttpClient { conn }
    }

    fn read_response(&mut self) -> Result<Response, String> {
        let mut r = BufReader::new(&mut self.conn);
        let mut buf = Vec::new();

        // read status line
        r.read_until(b'\n', &mut buf).unwrap();
        let status_line = String::from_utf8(buf.clone())
            .map_err(|_| "cannot convert bytes to string".to_string())?;

        let status = status_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| "cannot get status code".to_string())?
            .parse::<u32>()
            .map_err(|_| "cannot parse to number".to_string())?;

        // read headers
        let mut header = HttpHeader(BTreeMap::new());
        loop {
            buf.clear();
            let readed = r
                .read_until(b'\n', &mut buf)
                .map_err(|_| "cannot read header".to_string())?;

            if readed == 0 {
                return Err("unexpected endof".to_string());
            }

            let line = String::from_utf8(buf.clone())
                .map_err(|_| "cannot coonvert bytes to string".to_string())?;
            if line == "\r\n" {
                break;
            }
            let mut cols = line.split_whitespace();
            let key = cols
                .next()
                .ok_or_else(|| "invalid header key".to_string())?
                .to_string();
            let val = cols
                .next()
                .ok_or_else(|| "invalid header value".to_string())?
                .to_string()
                .to_string();
            header.0.insert(key, MapKey::String(val));
        }

        match status {
            204 | 304 => {
                let resp = Response {
                    status,
                    header,
                    body: None,
                };
                return Ok(resp);
            }
            _ => {}
        }

        // read body
        let mut body = Vec::new();
        loop {
            buf.clear();
            let readed = r.read_until(b'\n', &mut buf).unwrap();
            if readed == 0 {
                break;
            }

            let line = String::from_utf8(buf.clone())
                .map_err(|_| "cannot coonvert bytes to string".to_string())?;
            let chunk_size = i64::from_str_radix(line.trim(), 16).map_err(|err| {
                format!("cannot read chunk length: {}: {}", line, err).to_string()
            })?;

            if chunk_size == 0 {
                r.read_until(b'\n', &mut buf);
                break;
            }

            let mut chunk = vec![0u8; chunk_size as usize];
            r.read_exact(&mut chunk).unwrap();
            body.append(&mut chunk);

            // consume \r\n
            r.read_until(b'\n', &mut buf);
        }
        let resp = Response {
            status,
            header,
            body: Some(body),
        };
        Ok(resp)
    }

    fn request(&mut self, req: &mut Request) -> Result<Response, String> {
        let body = req.build();
        self.conn.write_all(&body).unwrap();
        self.read_response()
    }
}

fn main() -> std::io::Result<()> {
    let conn = UnixStream::connect("/var/run/docker.sock")?;
    let mut client = HttpClient::new(conn);
    let mut req = Request {
        base_url: Some("localhost".to_string()),
        url: "/images/json".to_string(),
        method: HttpMethod::Get,
        header: None,
        params: None,
        body: None,
    };
    let resp = client.request(&mut req).unwrap();
    print!("{}", String::from_utf8(resp.body.unwrap()).unwrap());
    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn request_build() {
        let mut req = Request {
            url: "/images/json".to_string(),
            method: HttpMethod::Get,
            ..Default::default()
        };
        let want = ["GET /images/json HTTP/1.1", "Host: localhost", "", ""].join("\r\n");
        let got = String::from_utf8(req.build()).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn request_get() {
        let mut req = Request::get("/images/json".to_string());
        let want = ["GET /images/json HTTP/1.1", "Host: localhost", "", ""].join("\r\n");
        let got = String::from_utf8(req.build()).unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn request_with_options() {
        let mut req = Request::new("/images/json".into());
        let params: HttpParams = [
            ("name", MapKey::String("nvim".into())),
            ("image", MapKey::String("ubuntu".into())),
        ]
        .into_iter()
        .collect();

        let mut header: HttpHeader = [
            ("bar", MapKey::Number(1000)),
            ("foo", MapKey::String("value".into())),
        ]
        .into_iter()
        .collect();

        let body = "test body".to_string().as_bytes().to_vec();

        req.method(HttpMethod::Get)
            .params(params)
            .header(header)
            .body(body);

        let want = [
            "GET /images/json?image=ubuntu&name=nvim HTTP/1.1",
            "Host: localhost",
            "bar: 1000",
            "foo: value",
            "",
            "test body",
            "",
        ]
        .join("\r\n");
        let got = String::from_utf8(req.build()).unwrap();
        assert_eq!(want, got);
    }
}
