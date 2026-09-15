//! Native SRV/TXT discovery on turnloop's bounded blocking pool. WASI 0.2
//! exposes A/AAAA via ip-name-lookup; it does not expose SRV/TXT records.
use crate::{Backend, ExecutorHandle, Instant, deadline};
use std::io;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Srv {
        target: String,
        port: u16,
        priority: u16,
        weight: u16,
        ttl: u32,
    },
    Txt {
        text: String,
        ttl: u32,
    },
}
#[derive(Clone, Copy)]
pub enum Query {
    Srv,
    Txt,
}
#[cfg(any(test, windows, all(unix, not(target_arch = "wasm32"))))]
impl Query {
    fn code(self) -> u16 {
        match self {
            Self::Srv => 33,
            Self::Txt => 16,
        }
    }
}
pub async fn query<B: Backend>(
    executor: &ExecutorHandle<B>,
    name: String,
    kind: Query,
    at: Instant,
) -> io::Result<Vec<Record>> {
    let result = deadline(executor, at, async {
        executor
            .blocking(move || {
                Ok(turnloop::Payload::Boxed(Box::new(native_query(
                    &name, kind,
                ))))
            })
            .await
            .map_err(crate::error)
    })
    .await?;
    match result {
        turnloop::Payload::Boxed(result) => *result
            .downcast::<io::Result<Vec<Record>>>()
            .map_err(|_| io::Error::other("invalid DNS worker result"))?,
        _ => Err(io::Error::other("invalid DNS worker payload")),
    }
}
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn native_query(name: &str, kind: Query) -> io::Result<Vec<Record>> {
    use std::sync::{
        Mutex,
        atomic::{AtomicU16, Ordering},
    };
    // Serialize implementations whose resolver state is process-global. Only
    // blocking workers take this mutex; the event-loop thread never does.
    static RESOLVER: Mutex<()> = Mutex::new(());
    static ID: AtomicU16 = AtomicU16::new(1);
    #[link(name = "resolv")]
    unsafe extern "C" {
        #[cfg_attr(target_vendor = "apple", link_name = "res_9_send")]
        fn res_send(
            msg: *const u8,
            msglen: std::ffi::c_int,
            answer: *mut u8,
            answerlen: std::ffi::c_int,
        ) -> std::ffi::c_int;
    }
    let mut packet = Vec::with_capacity(512);
    packet.extend_from_slice(&ID.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    packet.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 || !label.is_ascii() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    if packet.len() > 266 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    packet.push(0);
    packet.extend_from_slice(&kind.code().to_be_bytes());
    packet.extend_from_slice(&[0, 1]);
    let mut answer = vec![0; 65535];
    let _lock = RESOLVER
        .lock()
        .map_err(|_| io::Error::other("resolver lock poisoned"))?;
    // SAFETY: both buffers are live for this synchronous call, with lengths
    // bounded below c_int::MAX. The resolver copies data and retains no pointers.
    let n = unsafe {
        res_send(
            packet.as_ptr(),
            packet.len() as _,
            answer.as_mut_ptr(),
            answer.len() as _,
        )
    };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize > answer.len() {
        return Err(io::Error::other("truncated DNS packet"));
    }
    answer.truncate(n as usize);
    if answer.get(..2) != packet.get(..2) {
        return Err(io::Error::other("DNS query identity mismatch"));
    }
    parse(&answer, kind)
}
#[cfg(windows)]
fn native_query(name: &str, kind: Query) -> io::Result<Vec<Record>> {
    use std::ffi::{CStr, CString};
    use windows_sys::Win32::NetworkManagement::Dns::*;
    let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut first = std::ptr::null_mut();
    // SAFETY: NUL-terminated input and valid out-pointer, no custom server list.
    let status = unsafe {
        DnsQuery_UTF8(
            name.as_ptr().cast(),
            kind.code(),
            DNS_QUERY_STANDARD,
            std::ptr::null_mut(),
            &mut first,
            std::ptr::null_mut(),
        )
    };
    if status == 9003 || status == 9501 {
        return Ok(Vec::new());
    }
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    struct Records(*mut DNS_RECORDA);
    impl Drop for Records {
        fn drop(&mut self) {
            // SAFETY: this list is owned by the successful DnsQuery_UTF8 call.
            unsafe {
                DnsFree(self.0.cast(), DnsFreeRecordList);
            }
        }
    }
    let records = Records(first);
    let mut p = records.0;
    let mut output = Vec::new();
    while !p.is_null() {
        // SAFETY: walking the OS-owned, valid linked list before Records drops.
        let r = unsafe { &*p };
        if r.wType == 33 {
            // SAFETY: wType selects SRV data; the OS owns a NUL-terminated UTF-8 name.
            let srv = unsafe { &r.Data.SRV };
            if srv.pNameTarget.is_null() {
                return Err(io::Error::other("empty SRV target"));
            }
            // SAFETY: validated non-null target pointer in the live SRV record.
            let target = unsafe { CStr::from_ptr(srv.pNameTarget.cast()) }
                .to_str()
                .map_err(io::Error::other)?
                .to_owned();
            output.push(Record::Srv {
                target,
                port: srv.wPort,
                priority: srv.wPriority,
                weight: srv.wWeight,
                ttl: r.dwTtl,
            });
        } else if r.wType == 16 {
            // SAFETY: wType selects TXT; the OS allocates dwStringCount pointers
            // in this flexible array, all valid until the list is freed.
            let txt = unsafe { &r.Data.TXT };
            // SAFETY: flexible array extent is supplied by DnsQuery_UTF8.
            let strings = unsafe {
                std::slice::from_raw_parts(txt.pStringArray.as_ptr(), txt.dwStringCount as usize)
            };
            let mut text = String::new();
            for &ptr in strings {
                if ptr.is_null() {
                    return Err(io::Error::other("empty TXT pointer"));
                }
                // SAFETY: live non-null UTF-8 string owned by the DNS result.
                text.push_str(
                    unsafe { CStr::from_ptr(ptr.cast()) }
                        .to_str()
                        .map_err(io::Error::other)?,
                );
            }
            output.push(Record::Txt { text, ttl: r.dwTtl });
        }
        p = r.pNext;
    }
    Ok(output)
}
#[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
fn native_query(_: &str, _: Query) -> io::Result<Vec<Record>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "host has no SRV/TXT resolver capability",
    ))
}
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
fn parse(bytes: &[u8], kind: Query) -> io::Result<Vec<Record>> {
    fn invalid() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "invalid DNS packet")
    }
    fn u16_at(b: &[u8], at: usize) -> io::Result<u16> {
        Ok(u16::from_be_bytes(
            b.get(at..at + 2)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?,
        ))
    }
    fn name(b: &[u8], at: &mut usize) -> io::Result<String> {
        let mut out = String::new();
        let mut p = *at;
        let mut jumped = false;
        for _ in 0..128 {
            let len = *b.get(p).ok_or_else(invalid)?;
            if len & 0xc0 == 0xc0 {
                let dest = usize::from(u16_at(b, p)? & 0x3fff);
                if dest >= p {
                    return Err(invalid());
                }
                if !jumped {
                    *at = p + 2;
                    jumped = true;
                }
                p = dest;
            } else if len == 0 {
                if !jumped {
                    *at = p + 1;
                }
                return Ok(out);
            } else {
                if len > 63 {
                    return Err(invalid());
                }
                p += 1;
                let label =
                    std::str::from_utf8(b.get(p..p + usize::from(len)).ok_or_else(invalid)?)
                        .map_err(|_| invalid())?;
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(label);
                if out.len() > 253 {
                    return Err(invalid());
                }
                p += usize::from(len);
            }
        }
        Err(invalid())
    }
    let flags = u16_at(bytes, 2)?;
    if flags & 0x8000 == 0 || flags & 0x0200 != 0 {
        return Err(invalid());
    }
    match flags & 0xf {
        0 => {}
        3 => return Ok(Vec::new()),
        _ => return Err(io::Error::other("DNS server failed query")),
    }
    let mut at = 12;
    for _ in 0..u16_at(bytes, 4)? {
        name(bytes, &mut at)?;
        at += 4;
    }
    let mut records = Vec::new();
    for _ in 0..u16_at(bytes, 6)? {
        name(bytes, &mut at)?;
        let ty = u16_at(bytes, at)?;
        let class = u16_at(bytes, at + 2)?;
        let ttl = u32::from_be_bytes(
            bytes
                .get(at + 4..at + 8)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?,
        );
        let size = usize::from(u16_at(bytes, at + 8)?);
        at += 10;
        let end = at.checked_add(size).ok_or_else(invalid)?;
        let data = bytes.get(at..end).ok_or_else(invalid)?;
        if class == 1 && ty == kind.code() {
            if ty == 33 {
                if size < 7 {
                    return Err(invalid());
                }
                let priority = u16_at(bytes, at)?;
                let weight = u16_at(bytes, at + 2)?;
                let port = u16_at(bytes, at + 4)?;
                let mut cursor = at + 6;
                let target = name(bytes, &mut cursor)?;
                if cursor != end || target.is_empty() || port == 0 {
                    return Err(invalid());
                }
                records.push(Record::Srv {
                    target,
                    port,
                    priority,
                    weight,
                    ttl,
                });
            } else {
                let mut text = Vec::new();
                let mut p = 0;
                while p < data.len() {
                    let n = usize::from(data[p]);
                    p += 1;
                    text.extend_from_slice(data.get(p..p + n).ok_or_else(invalid)?);
                    p += n;
                }
                records.push(Record::Txt {
                    text: String::from_utf8(text).map_err(|_| invalid())?,
                    ttl,
                });
            }
        }
        at = end;
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn srv_txt_compression_and_malformed_answers() {
        let mut answer = vec![0, 1, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        answer.extend_from_slice(b"\x01x\x07example\x03com\0\0\x21\0\x01\xc0\x0c\0\x21\0\x01\0\0\0\x3c\0\x0b\0\x01\0\x02\x69\x89\x02db\xc0\x0e");
        assert_eq!(
            parse(&answer, Query::Srv).expect("SRV"),
            vec![Record::Srv {
                target: "db.example.com".into(),
                port: 27017,
                priority: 1,
                weight: 2,
                ttl: 60
            }]
        );
        assert!(parse(&answer[..answer.len() - 1], Query::Srv).is_err());
        let mut txt = vec![0, 1, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 0];
        txt.extend_from_slice(b"\0\0\x10\0\x01\0\0\0\x3c\0\x06\x02ab\x02cd");
        assert_eq!(
            parse(&txt, Query::Txt).expect("TXT"),
            vec![Record::Txt {
                text: "abcd".into(),
                ttl: 60
            }]
        );
        let mut looped = answer.clone();
        let n = looped.len();
        looped[n - 2] = 0xc0;
        looped[n - 1] = (n - 2) as u8;
        assert!(parse(&looped, Query::Srv).is_err());
        txt[3] = 2;
        assert!(parse(&txt, Query::Txt).is_err());
        txt[3] = 3;
        assert!(parse(&txt, Query::Txt).expect("NXDOMAIN").is_empty());
    }
}
