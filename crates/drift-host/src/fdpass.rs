//! Socket descriptors across a Unix socket (`SCM_RIGHTS`) and the peer's uid (`getpeereid`)
//! (task **HOST-13**). Humble object over libc.
//!
//! Each frame is a big-endian `u32` length and a payload, sent in one `sendmsg`; a descriptor,
//! when there is one, rides in the same call. The daemon hands an accepted TCP connection to an
//! agent this way, so the agent serves the client directly and the daemon never reads its bytes.

use std::io::{self, Read as _};
use std::mem::size_of;
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd};
use std::os::unix::net::UnixStream;

use crate::ipc::{self, MAX_FRAME, Message};

fn cmsg_space() -> usize {
    // SAFETY: CMSG_SPACE is a pure size computation.
    unsafe { libc::CMSG_SPACE(size_of::<libc::c_int>() as u32) as usize }
}

/// Sends `payload` as one frame, with `fd` attached when given.
pub fn send_with_fd(sock: &UnixStream, payload: &[u8], fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
    if payload.len() > MAX_FRAME + 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
    }
    let mut frame = u32::try_from(payload.len()).unwrap_or(u32::MAX).to_be_bytes().to_vec();
    frame.extend_from_slice(payload);
    let mut iov = libc::iovec { iov_base: frame.as_mut_ptr().cast(), iov_len: frame.len() };
    let mut control = vec![0_u8; cmsg_space()];
    // SAFETY: zeroed msghdr is a valid empty header; every pointer set below outlives the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len() as libc::socklen_t;
        // SAFETY: the control buffer is CMSG_SPACE(sizeof(int)) bytes, enough for one header and
        // one descriptor; CMSG_FIRSTHDR/CMSG_DATA point inside it.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<libc::c_int>() as u32) as libc::socklen_t;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>(), fd.as_raw_fd());
        }
    }
    let mut sent = 0;
    while sent < frame.len() {
        if sent > 0 {
            // The descriptor went with the first bytes; the rest is plain data.
            msg.msg_control = std::ptr::null_mut();
            msg.msg_controllen = 0;
            let rest = &mut frame[sent..];
            iov = libc::iovec { iov_base: rest.as_mut_ptr().cast(), iov_len: rest.len() };
            msg.msg_iov = &mut iov;
        }
        // SAFETY: `msg` describes live buffers; the socket is open.
        let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        sent += n as usize;
    }
    Ok(())
}

/// Receives one frame and the descriptor that came with it, if any. A closed socket is
/// `UnexpectedEof`.
pub fn recv_with_fd(sock: &UnixStream) -> io::Result<(Vec<u8>, Option<OwnedFd>)> {
    let mut header = [0_u8; 4];
    let mut iov = libc::iovec { iov_base: header.as_mut_ptr().cast(), iov_len: header.len() };
    let mut control = vec![0_u8; cmsg_space()];
    // SAFETY: as in `send_with_fd`.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len() as libc::socklen_t;
    let n = loop {
        // SAFETY: `msg` describes live buffers; the socket is open.
        let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        break n as usize;
    };
    if n == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let mut fd = None;
    if msg.msg_controllen as usize >= size_of::<libc::cmsghdr>() {
        // SAFETY: the kernel filled `control` and set msg_controllen; we only read headers it wrote.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            if !cmsg.is_null()
                && (*cmsg).cmsg_level == libc::SOL_SOCKET
                && (*cmsg).cmsg_type == libc::SCM_RIGHTS
            {
                let raw = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>());
                fd = Some(OwnedFd::from_raw_fd(raw));
            }
        }
    }
    let mut reader = sock;
    if n < header.len() {
        reader.read_exact(&mut header[n..])?;
    }
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME + 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut payload = vec![0_u8; len];
    reader.read_exact(&mut payload)?;
    Ok((payload, fd))
}

/// Sends an IPC message, with a descriptor for [`Message::Connection`].
pub fn send_message(sock: &UnixStream, message: &Message, fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
    let frame = ipc::encode(message);
    send_with_fd(sock, &frame[4..], fd)
}

/// Receives an IPC message and its descriptor.
pub fn recv_message(sock: &UnixStream) -> io::Result<(Message, Option<OwnedFd>)> {
    let (payload, fd) = recv_with_fd(sock)?;
    let message = ipc::decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((message, fd))
}

/// The effective uid of the process at the other end of `sock`.
pub fn peer_uid(sock: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid writes two integers; the socket is open.
    if unsafe { libc::getpeereid(sock.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
