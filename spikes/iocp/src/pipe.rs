use crate::{
    operation::{Endpoint, Operation, drain},
    port::{Entry, Port, Wait, bool_result, owned},
};
use std::{
    io,
    os::windows::io::OwnedHandle,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::*,
    System::Pipes::*,
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
pub(crate) fn pipe_name() -> Vec<u16> {
    format!(
        "\\\\.\\pipe\\turnloop-iocp-{}-{}\0",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
    .encode_utf16()
    .collect()
}

pub(crate) fn server(name: &[u16], access: u32) -> io::Result<OwnedHandle> {
    assert_eq!(name.last(), Some(&0));
    // SAFETY: terminated name and no security pointer. Overlapped PIPE_WAIT, not
    // PIPE_NOWAIT (the latter is a legacy polling mode). Reject remote clients.
    // https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew
    unsafe {
        owned(CreateNamedPipeW(
            name.as_ptr(),
            access | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            ptr::null(),
        ))
    }
}

pub(crate) fn client(name: &[u16], access: u32, overlapped: bool) -> io::Result<OwnedHandle> {
    assert_eq!(name.last(), Some(&0));
    // SAFETY: terminated local pipe name; newly owned client with matching access.
    unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            access,
            0,
            ptr::null(),
            OPEN_EXISTING,
            if overlapped { FILE_FLAG_OVERLAPPED } else { 0 },
            ptr::null_mut(),
        ))
    }
}

pub(crate) fn connect(op: &mut Operation) -> io::Result<()> {
    let (overlapped, _) = op.prepare()?;
    // SAFETY: overlapped server, stable op and manual-reset event.
    // ERROR_PIPE_CONNECTED is successful connection WITHOUT a queued I/O packet.
    // https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe
    let ok = unsafe { ConnectNamedPipe(op.endpoint.raw(), overlapped) };
    let error = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    op.submitted(
        ok != 0 || error == ERROR_PIPE_CONNECTED as i32,
        error,
        0,
        true,
    )
}

pub(crate) fn read(op: &mut Operation) -> io::Result<()> {
    let (overlapped, buffer) = op.prepare()?;
    let mut bytes = 0;
    // SAFETY: op-owned buffer is writable and remains stable until completion.
    // https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile
    let ok = unsafe { ReadFile(op.endpoint.raw(), buffer, 512, &mut bytes, overlapped) };
    op.submitted(
        ok != 0,
        io::Error::last_os_error().raw_os_error().unwrap_or(0),
        bytes,
        true,
    )
}

pub(crate) fn write(op: &mut Operation, data: &[u8]) -> io::Result<()> {
    op.set_data(data);
    let (overlapped, buffer) = op.prepare()?;
    let mut bytes = 0;
    // SAFETY: bytes copied into stable storage and remain live through completion.
    // https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-writefile
    let ok = unsafe {
        WriteFile(
            op.endpoint.raw(),
            buffer,
            data.len() as u32,
            &mut bytes,
            overlapped,
        )
    };
    op.submitted(
        ok != 0,
        io::Error::last_os_error().raw_os_error().unwrap_or(0),
        bytes,
        true,
    )
}

pub(crate) fn associate(port: &Port, endpoint: &Endpoint, key: usize) -> io::Result<()> {
    // SAFETY: fresh overlapped named-pipe handle; only associated with this port.
    unsafe { port.associate(endpoint.raw(), key) }?;
    // SAFETY: pipe supports IFS notification modes; permanent skip-success flag.
    bool_result(unsafe { SetFileCompletionNotificationModes(endpoint.raw(), 1) })
}

pub fn probe(client_first: bool) -> io::Result<usize> {
    let port = Port::new()?;
    let name = pipe_name();
    let server = Rc::new(Endpoint::Pipe(server(&name, PIPE_ACCESS_DUPLEX)?));
    associate(&port, &server, 10)?;
    let mut connecting = Operation::new(Rc::clone(&server))?;
    let client = if client_first {
        let client = client(&name, GENERIC_READ | GENERIC_WRITE, true)?;
        connect(&mut connecting)?;
        assert!(!connecting.pending);
        client
    } else {
        connect(&mut connecting)?;
        assert!(connecting.pending);
        client(&name, GENERIC_READ | GENERIC_WRITE, true)?
    };
    let client = Rc::new(Endpoint::Pipe(client));
    associate(&port, &client, 11)?;
    drain(&port, &mut [&mut connecting])?;
    assert_eq!(connecting.completions, 1);
    let mut reading = Operation::new(Rc::clone(&server))?;
    let mut writing = Operation::new(Rc::clone(&client))?;
    let mut echoed = 0;
    for _ in 0..128 {
        read(&mut reading)?;
        assert!(reading.pending);
        write(&mut writing, b"named-pipe")?;
        drain(&port, &mut [&mut reading, &mut writing])?;
        assert_eq!(reading.result, Some((0, 10)));
        assert_eq!(writing.result, Some((0, 10)));
        assert_eq!(&reading.data()[..10], b"named-pipe");
        echoed += 10;
    }
    read(&mut reading)?;
    reading.cancel()?;
    drain(&port, &mut [&mut reading])?;
    assert_eq!(reading.result.map(|r| r.0), Some(0xc0000120u32 as i32));
    drop(reading);
    drop(connecting);
    drop(server);
    let mut entries = [Entry::default(); 4];
    assert_eq!(
        port.wait(Some(Duration::from_millis(10)), false, &mut entries)?,
        Wait::Timeout
    );
    Ok(echoed)
}
