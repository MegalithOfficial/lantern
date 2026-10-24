//! Checks published Lantern releases. Minecraft versions live in `versions`.

use std::{io::Read, time::Duration};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    UpToDate,
    Available { version: String, url: String },
    NoRelease,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

impl Release {
    fn status(self, current: &str) -> Result<Status> {
        ensure!(!self.draft && !self.prerelease, "GitHub returned an unpublished or preview release");
        let latest = Version::parse(self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name)).context("reading the release version")?;
        let current = Version::parse(current).context("reading the installed version")?;
        Ok(if latest.cmp_precedence(&current).is_gt() {
            Status::Available { version: latest.to_string(), url: format!("{}/releases/tag/{}", crate::REPOSITORY, self.tag_name) }
        } else {
            Status::UpToDate
        })
    }
}

pub fn check() -> Result<Status> {
    let repository = crate::REPOSITORY.strip_prefix("https://github.com/").context("the release repository is not a GitHub URL")?;
    check_url(&format!("https://api.github.com/repos/{repository}/releases/latest"))
}

fn check_url(url: &str) -> Result<Status> {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).timeout(Duration::from_secs(15)).build();
    let response = match agent
        .get(url)
        .set("User-Agent", &format!("Lantern/{}", crate::VERSION))
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(Status::NoRelease),
        Err(err) => return Err(err).context("checking GitHub for a newer Lantern release"),
    };
    let release: Release = serde_json::from_reader(response.into_reader().take(1024 * 1024)).context("reading GitHub's release response")?;
    release.status(crate::VERSION)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    use super::*;

    fn release(tag: &str) -> Release {
        Release { tag_name: tag.to_owned(), draft: false, prerelease: false }
    }

    #[test]
    fn compares_semantic_versions_and_ignores_build_metadata() {
        assert!(matches!(release("v0.10.0").status("0.9.0").unwrap(), Status::Available { .. }));
        assert_eq!(release("v1.2.3").status("1.2.3+local").unwrap(), Status::UpToDate);
        assert_eq!(release("v1.2.3").status("1.3.0").unwrap(), Status::UpToDate);
        assert!(matches!(release("1.2.3").status("1.2.3-rc.1").unwrap(), Status::Available { .. }));
    }

    #[test]
    fn rejects_preview_draft_and_invalid_versions() {
        let mut preview = release("v1.0.0-rc.1");
        preview.prerelease = true;
        assert!(preview.status("0.1.0").is_err());
        let mut draft = release("v1.0.0");
        draft.draft = true;
        assert!(draft.status("0.1.0").is_err());
        assert!(release("latest").status("0.1.0").is_err());
    }

    #[test]
    fn handles_http_release_missing_release_and_server_errors() {
        for (status, body) in [
            ("200 OK", r#"{"tag_name":"v999.0.0","draft":false,"prerelease":false}"#),
            ("404 Not Found", "{}"),
            ("403 Forbidden", "{}"),
            ("200 OK", "invalid json"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/latest", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    assert_eq!(stream.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.contains("user-agent: lantern/"));
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = check_url(&url);
            server.join().unwrap();
            match status {
                "200 OK" if body.starts_with('{') => {
                    assert_eq!(
                        result.unwrap(),
                        Status::Available { version: "999.0.0".to_owned(), url: format!("{}/releases/tag/v999.0.0", crate::REPOSITORY) }
                    );
                }
                "404 Not Found" => assert_eq!(result.unwrap(), Status::NoRelease),
                _ => assert!(result.is_err()),
            }
        }
    }
}
