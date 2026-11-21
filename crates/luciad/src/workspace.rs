//! 상주 VM의 작업 공간과 파일 보관소를 잇는다(결정 0059).
//!
//! 작업 공간은 호스트의 디렉터리이고 VM에 쓰기 가능하게 마운트되어 있다. Lucia가 쉘로 무엇이든 쓸 수 있으므로,
//! **호스트가 그 안의 경로를 열 때는 VM이 심어 둔 심볼릭 링크를 따라가지 않아야 한다.** 따라가면 VM이
//! `작업 공간/x → /home/…/.ssh/id_ed25519` 같은 링크로 호스트의 파일을 읽게 하거나(가져오기), 호스트의 아무 곳에
//! 쓰게 할 수 있다(내보내기).
//!
//! 그래서 경로를 조각마다 `openat(O_NOFOLLOW)`로 내려간다. 어느 조각이든 링크면 거절한다. 검사하고 여는 사이에
//! 바꿔치기해도, 연 것은 링크가 아닌 그 디렉터리 자체다.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};

/// 가져오거나 내보내는 파일 하나의 크기.
pub const MAX_BYTES: u64 = crate::downloads::MAX_BYTES;
/// 작업 공간이 이만큼 커지면 정리하라고 알린다 [제안].
pub const WARN_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// 크기를 셀 때 보는 항목 수 상한(그 뒤는 세지 않는다).
const USAGE_ENTRIES_MAX: usize = 200_000;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

fn last_error(what: &str) -> String {
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
            format!("{what}: 심볼릭 링크이거나 디렉터리가 아니다(링크는 따라가지 않는다)")
        }
        Some(libc::ENOENT) => format!("{what}: 그런 파일이 없다"),
        _ => format!("{what}: {err}"),
    }
}

fn c(name: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(name.as_bytes()).map_err(|_| "경로에 쓸 수 없는 글자가 있다".to_owned())
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 작업 공간 안의 경로를 조각으로. 절대 경로면 작업 공간 아래여야 한다. `..`는 받지 않는다.
    fn parts(&self, path: &str) -> Result<Vec<std::ffi::OsString>, String> {
        let given = Path::new(path.trim());
        let rel = if given.is_absolute() {
            given.strip_prefix(&self.root).map_err(|_| {
                format!(
                    "작업 공간({}) 밖의 경로다. 작업 공간 안의 파일만 다룬다",
                    self.root.display()
                )
            })?
        } else {
            given
        };
        let mut parts = Vec::new();
        for comp in rel.components() {
            match comp {
                Component::Normal(name) => parts.push(name.to_owned()),
                Component::CurDir => {}
                _ => return Err("경로에 ..를 쓸 수 없다".into()),
            }
        }
        if parts.is_empty() {
            return Err("파일 경로가 비었다".into());
        }
        Ok(parts)
    }

    /// 작업 공간에서 `dirs`를 따라 내려간 디렉터리를 연다(링크를 따라가지 않는다). `create`면 없는 것을 만든다.
    fn open_dir(&self, dirs: &[std::ffi::OsString], create: bool) -> Result<OwnedFd, String> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let root = c(self.root.as_os_str())?;
        // SAFETY: 널로 끝나는 경로와 고정 플래그로 open(2)을 부른다. 돌려받은 fd는 바로 OwnedFd가 가진다.
        let fd = unsafe { libc::open(root.as_ptr(), flags) };
        if fd < 0 {
            return Err(last_error("작업 공간"));
        }
        // SAFETY: 방금 연, 아무도 갖지 않은 fd다.
        let mut dir = unsafe { OwnedFd::from_raw_fd(fd) };
        for name in dirs {
            let cname = c(name)?;
            if create {
                // SAFETY: 살아 있는 디렉터리 fd와 널로 끝나는 이름으로 mkdirat(2)을 부른다. 이미 있으면 실패해도 된다.
                unsafe { libc::mkdirat(dir.as_raw_fd(), cname.as_ptr(), 0o755) };
            }
            // SAFETY: 살아 있는 디렉터리 fd와 널로 끝나는 이름으로 openat(2)을 부른다.
            let next = unsafe { libc::openat(dir.as_raw_fd(), cname.as_ptr(), flags) };
            if next < 0 {
                return Err(last_error(&name.to_string_lossy()));
            }
            // SAFETY: 방금 연, 아무도 갖지 않은 fd다.
            dir = unsafe { OwnedFd::from_raw_fd(next) };
        }
        Ok(dir)
    }

    /// 작업 공간의 일반 파일 하나를 읽는다: (이름, 내용).
    pub fn read(&self, path: &str) -> Result<(String, Vec<u8>), String> {
        let parts = self.parts(path)?;
        let (name, dirs) = parts.split_last().expect("비어 있지 않다");
        let dir = self.open_dir(dirs, false)?;
        let cname = c(name)?;
        // 이름 붙은 파이프 같은 것을 열다 멈추지 않게 O_NONBLOCK으로 연다.
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: 살아 있는 디렉터리 fd와 널로 끝나는 이름으로 openat(2)을 부른다.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), cname.as_ptr(), flags) };
        if fd < 0 {
            return Err(last_error(&name.to_string_lossy()));
        }
        // SAFETY: 방금 연, 아무도 갖지 않은 fd다.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("일반 파일이 아니다(디렉터리나 장치는 가져오지 않는다)".into());
        }
        if meta.len() > MAX_BYTES {
            return Err(format!(
                "파일이 너무 크다({}까지)",
                crate::files::human_size(MAX_BYTES)
            ));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        (&mut file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(format!(
                "파일이 너무 크다({}까지)",
                crate::files::human_size(MAX_BYTES)
            ));
        }
        Ok((name.to_string_lossy().into_owned(), bytes))
    }

    /// 작업 공간에 파일을 쓴다(있으면 덮어쓴다). 쓴 곳의 절대 경로를 돌려준다.
    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let parts = self.parts(path)?;
        let (name, dirs) = parts.split_last().expect("비어 있지 않다");
        let dir = self.open_dir(dirs, true)?;
        let cname = c(name)?;
        // O_NONBLOCK: VM이 그 자리에 이름 붙은 파이프를 두어도 여는 데서 멈추지 않는다(읽는 쪽이 없으면 실패한다).
        let flags = libc::O_WRONLY
            | libc::O_CREAT
            | libc::O_TRUNC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC;
        // SAFETY: 살아 있는 디렉터리 fd와 널로 끝나는 이름으로 openat(2)을 부른다. 모드는 고정이다.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), cname.as_ptr(), flags, 0o644) };
        if fd < 0 {
            return Err(last_error(&name.to_string_lossy()));
        }
        // SAFETY: 방금 연, 아무도 갖지 않은 fd다.
        let mut file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("일반 파일이 아닌 곳에는 쓰지 않는다".into());
        }
        file.write_all(bytes).map_err(|e| e.to_string())?;
        let mut out = self.root.clone();
        out.extend(parts.iter());
        Ok(out)
    }

    /// 작업 공간이 차지한 크기(링크를 따라가지 않는다). 항목이 너무 많으면 센 데까지.
    pub fn usage(&self) -> u64 {
        let mut total = 0u64;
        let mut seen = 0usize;
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                seen += 1;
                if seen > USAGE_ENTRIES_MAX {
                    return total;
                }
                // `DirEntry::metadata`는 링크를 따라가지 않는다.
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    total += meta.len();
                }
            }
        }
        total
    }

    /// 작업 공간이 커졌으면 정리하라는 말.
    pub fn usage_note(&self) -> String {
        let used = self.usage();
        if used >= WARN_BYTES {
            format!(
                "\n작업 공간이 {}다(호스트의 디스크를 쓴다). 필요 없는 파일을 지운다.",
                crate::files::human_size(used)
            )
        } else {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> (Workspace, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("lucia-workspace-{}", uuid::Uuid::new_v4().simple()));
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        (Workspace::new(root), base)
    }

    #[test]
    fn reads_and_writes_stay_inside_and_never_follow_links() {
        let (w, base) = space();
        let root = w.root().to_owned();
        // 호스트의 비밀이라고 치자(작업 공간 밖).
        std::fs::write(base.join("secret.txt"), "호스트의 비밀").unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();

        // 보통 파일: 상대 경로와 절대 경로 모두, 없는 디렉터리는 만든다.
        let put = w.write("out/보고서.txt", "안녕".as_bytes()).unwrap();
        assert_eq!(put, root.join("out/보고서.txt"));
        assert_eq!(
            w.read("out/보고서.txt").unwrap(),
            ("보고서.txt".into(), "안녕".as_bytes().to_vec())
        );
        assert_eq!(w.read(put.to_str().unwrap()).unwrap().1, "안녕".as_bytes());
        assert_eq!(w.read("./out/./보고서.txt").unwrap().0, "보고서.txt");
        w.write("out/보고서.txt", b"v2").unwrap();
        assert_eq!(w.read("out/보고서.txt").unwrap().1, b"v2");

        // 밖으로 나가는 경로.
        for bad in [
            "../secret.txt",
            "out/../../secret.txt",
            "/etc/passwd",
            "",
            ".",
        ] {
            assert!(w.read(bad).is_err(), "{bad}");
            assert!(w.write(bad, b"x").is_err(), "{bad}");
        }
        let outside = base.join("secret.txt");
        assert!(
            w.read(outside.to_str().unwrap())
                .unwrap_err()
                .contains("밖의 경로")
        );

        // VM이 심은 링크: 파일 링크, 디렉터리 링크, 중간 조각의 링크 모두 따라가지 않는다.
        std::os::unix::fs::symlink(base.join("secret.txt"), root.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), root.join("escape")).unwrap();
        std::os::unix::fs::symlink(&base, root.join("up")).unwrap();
        let err = w.read("leak.txt").unwrap_err();
        assert!(err.contains("링크"), "{err}");
        assert!(w.read("up/secret.txt").unwrap_err().contains("링크"));
        assert!(
            w.write("escape/planted.txt", b"x")
                .unwrap_err()
                .contains("링크")
        );
        assert!(
            w.write("leak.txt", b"overwrite")
                .unwrap_err()
                .contains("링크")
        );
        assert_eq!(
            std::fs::read_to_string(base.join("secret.txt")).unwrap(),
            "호스트의 비밀"
        );
        assert!(
            std::fs::read_dir(base.join("outside"))
                .unwrap()
                .next()
                .is_none()
        );

        // 이름 붙은 파이프를 두어도 멈추지 않고 거절한다(읽기와 쓰기 모두).
        let fifo = c(root.join("pipe").as_os_str()).unwrap();
        // SAFETY: 널로 끝나는 경로로 mkfifo(3)를 부른다.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        assert!(w.read("pipe").unwrap_err().contains("일반 파일이 아니다"));
        assert!(w.write("pipe", b"x").is_err());

        // 디렉터리와 없는 파일은 가져오지 않는다.
        assert!(w.read("out").unwrap_err().contains("일반 파일이 아니다"));
        assert!(w.read("out/없음.txt").unwrap_err().contains("없다"));

        // 크기는 링크를 따라가지 않고 센다.
        assert_eq!(w.usage(), 2);
        assert!(w.usage_note().is_empty());
        let _ = std::fs::remove_dir_all(base);
    }
}
