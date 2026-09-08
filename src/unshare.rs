use std::{
    ffi::OsStr, fs::File, io::Write, mem::forget, os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    }, path::PathBuf, process::Command, time::Duration,
};

use anyhow::{Context, bail};
use nix::{
    cmsg_space,
    errno::Errno,
    fcntl::{AT_FDCWD, OFlag, openat},
    libc,
    mount::{MntFlags, MsFlags, mount, umount2},
    poll,
    sched::{CloneFlags, unshare},
    sys::{
        signal,
        socket::{self, ControlMessage, ControlMessageOwned, recvmsg, sendmsg, socketpair},
        stat::{Mode, umask},
    },
    unistd::{ForkResult, Pid, chdir, fchdir, fork, mkdir, pivot_root, unlink},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcMessage {
    Closed,
    Started {
        pid: u32,
        stdin: Option<usize>,
        stdout: Option<usize>,
        stderr: Option<usize>,
    },
    LaunchFailed,
    AddInputDev(String),
    RemInputDev(String),
}

#[repr(transparent)]
struct AutoDieChild(std::process::Child);
impl Drop for AutoDieChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

struct RpcSocket {
    stream: UnixStream,
}

impl RpcSocket {
    pub fn new(stream: OwnedFd) -> Self {
        let stream = UnixStream::from(stream);
        stream
            .set_write_timeout(Some(Duration::from_millis(50)))
            .unwrap(); // Unwrap should never be hit
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap(); // Unwrap should never be hit
        RpcSocket { stream }
    }

    // Send message with a Vec of FDs
    pub fn send_message_with_fds(
        &mut self,
        msg: RpcMessage,
        fds: Vec<OwnedFd>,
    ) -> anyhow::Result<()> {
        let mut buf = Vec::new();
        postcard::to_io(&msg, &mut buf)?;

        let raw_fds: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();

        let cmsg = if !raw_fds.is_empty() {
            vec![ControlMessage::ScmRights(&raw_fds)]
        } else {
            vec![]
        };

        let iov = [std::io::IoSlice::new(&buf)];
        match sendmsg::<()>(
            self.stream.as_raw_fd(),
            &iov,
            &cmsg,
            nix::sys::socket::MsgFlags::empty(),
            None,
        ) {
            Ok(_) => Ok(()),
            Err(Errno::EPIPE) => Ok(()), // Ignore pipe closed errors, COMS are dead, but we really dont care.
            Err(e) => Err(e).context("Failed to send message with FDs"),
        }?;

        std::mem::forget(fds); // Don't close the FDs; ownership transferred
        Ok(())
    }

    // Receive message and automatically collect all FDs from control messages
    pub fn recv_message_with_fds(&mut self) -> anyhow::Result<(RpcMessage, Vec<OwnedFd>)> {
        let mut buf = [0u8; 2048];
        let mut cmsg_buf = cmsg_space!([RawFd; 16]); // Space for up to 16 FDs

        let mut iov = [std::io::IoSliceMut::new(&mut buf)];
        let msg = recvmsg::<()>(
            self.stream.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg_buf),
            nix::sys::socket::MsgFlags::empty(),
        ).context("Failed to receive message from socket")?;

        let msg_size = msg.bytes;

        // Collect all FDs from all control messages
        let mut fds = Vec::new();

        for cmsg in msg.cmsgs()? {
            if let ControlMessageOwned::ScmRights(raw_fds) = cmsg {
                for raw_fd in raw_fds {
                    fds.push(unsafe { OwnedFd::from_raw_fd(raw_fd) });
                }
            }
        }

        if msg_size == 0 {
            // Other side closed fd / EPIPE producing on write now.
            return Ok((RpcMessage::Closed, fds));
        }

        let msg_data = postcard::from_bytes::<RpcMessage>(&buf[..msg_size])?;

        Ok((msg_data, fds))
    }

    pub fn send_message(&mut self, msg: RpcMessage) -> anyhow::Result<()> {
        self.send_message_with_fds(msg, Vec::new())
    }

    pub fn recv_message(&mut self) -> anyhow::Result<RpcMessage> {
        Ok(self.recv_message_with_fds()?.0)
    }
}
impl Drop for RpcSocket {
    fn drop(&mut self) {
        let _ = self.send_message(RpcMessage::Closed);
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }
}

pub struct NamespaceSetup {
    pub cmd: Command,
    pub input_devs: Vec<String>,

}

pub struct RemoteNamespace {
    remote_socket: RpcSocket,

    // Namespace process PID, not the child inside the NS.
    namespace_child: Pid,
    namespace_child_pidfd: PidFD,

    pub child: Pid,
    pub child_pidfd: PidFD,
    pub child_stdin: Option<OwnedFd>,
    pub child_stdout: Option<OwnedFd>,
    pub child_stderr: Option<OwnedFd>,

    pub setup: NamespaceSetup,
}
impl RemoteNamespace {
    pub fn new(mut setup: NamespaceSetup) -> anyhow::Result<Self> {
        // socket::SockProtocol::NetlinkRoute IS WRONG, but it is == 0, so I just am using it rather than unsafe stuff.
        let sockets = socketpair(
            socket::AddressFamily::Unix,
            socket::SockType::Stream,
            socket::SockProtocol::NetlinkRoute,
            socket::SockFlag::empty(),
        ).context("Failed to create socketpair")?;

        match unsafe { fork()? } {
            ForkResult::Parent {
                child: namespace_child,
                ..
            } => {
                let namespace_child_pidfd = PidFD::open(namespace_child.as_raw() as u32)?;
                let mut remote_socket = RpcSocket::new(sockets.0);

                let message_reponse = remote_socket.recv_message_with_fds()?;

                if let (
                    RpcMessage::Started {
                        pid,
                        stdin,
                        stdout,
                        stderr,
                    },
                    fds,
                ) = message_reponse
                {
                    Ok(Self {
                        namespace_child,
                        namespace_child_pidfd,
                        remote_socket,

                        child: Pid::from_raw(pid as libc::pid_t),
                        child_pidfd: PidFD::open_from_fd(&fds[0])?,
                        child_stdin: stdin.map(|f| fds[f].try_clone().unwrap()),
                        child_stdout: stdout.map(|f| fds[f].try_clone().unwrap()),
                        child_stderr: stderr.map(|f| fds[f].try_clone().unwrap()),

                        setup,
                    })
                } else {
                    // signal::kill(namespace_child, signal::Signal::SIGKILL).context("Failed to kill namespace child")?;
                    bail!(
                        "Unknown response setting up namespace: {:?}",
                        message_reponse
                    );
                }
            }
            ForkResult::Child => {
                // Unsafe to use `println!` (or `unwrap`) here. See Safety.
                // write(std::io::stdout(), "Correct way to log stuff without crashing\n".as_bytes()).ok();
                let socket = RpcSocket::new(sockets.1);
                // UNWRAP "unsafe" but we dont care :D
                // fork_inner(&mut setup, socket).unwrap();

                // Turns out we do care, this will leave the `exit` syscall to fail, and the process to be a zombie with a fake open window.
                let _ = fork_inner(&mut setup, socket).inspect_err(|e| eprintln!("ERROR IN CONTAINER: {e:?}"));

                unsafe { libc::_exit(0) };
            }
        }
    }

    pub fn wait_container_exit(&mut self) -> anyhow::Result<()> {
        self.namespace_child_pidfd.wait()?;
        Ok(())
    }

    pub fn kill_ns(&mut self) -> anyhow::Result<()> {
        self.namespace_child_pidfd.signal(signal::Signal::SIGKILL)?;
        self.namespace_child_pidfd.wait()?;

        Ok(())
    }

    pub fn bind_device(&mut self, event: String) -> anyhow::Result<()> {
        self.remote_socket
            .send_message(RpcMessage::AddInputDev(event))
    }
    pub fn unbind_device(&mut self, event: String) -> anyhow::Result<()> {
        self.remote_socket
            .send_message(RpcMessage::RemInputDev(event))
    }
}

fn fork_inner(setup: &mut NamespaceSetup, mut socket: RpcSocket) -> anyhow::Result<()> {
    enter_new_namespace(setup)?;

    let mut child = AutoDieChild(setup.cmd
        .spawn()
        .inspect_err(|_| socket.send_message(RpcMessage::LaunchFailed).unwrap())?);
    let child_pid = child.0.id();

    let child_pidfd = PidFD::open(child_pid)?;

    {
        let mut fds = vec![child_pidfd.as_owned()?];

        let mut push_idx = |fd: BorrowedFd<'_>| {
            fds.push(fd.try_clone_to_owned().unwrap());
            fds.len() - 1
        };

        socket.send_message_with_fds(
            RpcMessage::Started {
                pid: child_pid,
                stdin: child.0.stdin.as_ref().map(|f| push_idx(f.as_fd())),
                stdout: child.0.stdout.as_ref().map(|f| push_idx(f.as_fd())),
                stderr: child.0.stderr.as_ref().map(|f| push_idx(f.as_fd())),
            },
            fds,
        ).context("Failed to send started message to container")?;
    }

    loop {
        let child_poll = poll::PollFd::new(child_pidfd.as_fd(), poll::PollFlags::POLLIN);
        let socket_poll = poll::PollFd::new(
            socket.stream.as_fd(),
            poll::PollFlags::POLLIN | poll::PollFlags::POLLHUP,
        );
        let mut pollfds = [child_poll, socket_poll];

        poll::poll(&mut pollfds, poll::PollTimeout::MAX).context("Failed to poll child and socket")?;

        // child_poll
        if child.0.try_wait()?.is_some() {
            println!("Child in container died, container exiting.");
            return Ok(());
        }

        // Cant use the name for socket_poll, because we moved it to the pollfds.
        if pollfds[1].revents().is_some() {
            match socket.recv_message_with_fds()? {
                (RpcMessage::Closed, _) => {
                    println!("Host died, container exiting.");
                    child.0.kill().context("Failed to kill child when host died")?;
                    return Ok(());
                }
                (RpcMessage::AddInputDev(device_path), _) => {
                    inner_bind_device(&device_path)?;
                }
                (RpcMessage::RemInputDev(device_path), _) => {
                    inner_unbind_device(&device_path)?;
                }
                _ => {
                    anyhow::bail!("Got sent some invalid command to container.");
                }
            }
        }
    }
}

fn inner_bind_device(device_path: &String) -> anyhow::Result<()> {
    let from_path =
        PathBuf::from("/tmp/PARTYDECK_HOST_DEV_INPUT_MOUNT").join(device_path);
    let to_path = PathBuf::from("/dev/input").join(device_path);
    println!("Bind request: {from_path:?} -> {to_path:?}");

    if !to_path.exists() {
        std::fs::File::create(&to_path).context("Failed to create device mount point")?;
    }

    // Command::spawn(Command::new("ls").arg("/tmp/PARTYDECK_HOST_DEV_INPUT_MOUNT"));

    // Prevents bugs if we try to bind the same path twice, the bind mount would fail if it was already bound.
    // This avoids that by unbinding or maybe not, we dont care :D
    let _ = umount2(&to_path, MntFlags::MNT_DETACH);

    mount(
        Some(&from_path),
        &to_path,
        NONE_STR,
        MsFlags::MS_BIND | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context(format!("Failed to bind mount device {device_path}"))?;
    
    Ok(())
}
fn inner_unbind_device(device_path: &String) -> anyhow::Result<()> {
    let to_path = PathBuf::from("/dev/input").join(device_path);
    println!("Unbind request: {to_path:?}, {}", to_path.exists());

    if to_path.exists() {
        umount2(&to_path, MntFlags::MNT_DETACH).context(format!("Failed to unmount device {device_path}"))?;
    }

    let _ = unlink(&to_path); // I really dont care, this can fail, doesnt affect much.

    Ok(())
}

fn enter_new_namespace(setup: &NamespaceSetup) -> anyhow::Result<()> {
    let old_uid = nix::unistd::getuid();
    let old_gid = nix::unistd::getgid();

    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS).context("Failed to unshare namespaces")?;

    write_uid_map(old_uid, old_gid)?;

    setup_roots()?;

    // Dev bind / /
    mount(
        Some("/oldroot"),
        "/newroot",
        NONE_STR,
        MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to bind mount /oldroot to /newroot (--dev-bind / /)")?;

    // Create fake /dev/input, and keep track of host's input so we can bind later.
    let _ = mkdir(
        "/newroot/tmp/PARTYDECK_HOST_DEV_INPUT_MOUNT",
        Mode::from_bits_retain(0o755),
    ); // Ignore if dir already created.

    mount(
        Some("/oldroot/dev/input"),
        "/newroot/tmp/PARTYDECK_HOST_DEV_INPUT_MOUNT",
        NONE_STR,
        MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to bind mount host /dev/input to temporary location")?;
    mount(
        NONE_STR,
        "/newroot/dev/input",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to mount tmpfs on /newroot/dev/input")?;

    mkdir("/newroot/dev/input/by-path", Mode::from_bits_retain(0o755)).context("Failed to create /dev/input/by-path")?;
    mkdir("/newroot/dev/input/by-id", Mode::from_bits_retain(0o755)).context("Failed to create /dev/input/by-id")?;

    mount(
        Some("/oldroot/dev/input/by-path"),
        "/newroot/dev/input/by-path",
        NONE_STR,
        MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to bind mount by-path")?;
    mount(
        Some("/oldroot/dev/input/by-id"),
        "/newroot/dev/input/by-id",
        NONE_STR,
        MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to bind mount by-id")?;











    // I havent had problems with this on host WL or X11 so I just assume its safe and avoids a lot of hastle patching 
    // the out of scope wlroots "issue" that for some reason checks who owns this directory and refuses to launch
    // if its not root or us, and seeing as root -> nobody:nobody, uid != 0. 
    mount(
        NONE_STR,
        "/newroot/tmp/.X11-unix",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to mount tmpfs on /newroot/tmp/.X11-unix")?;



    // TODO: mount overlayfs.

    cleanup_enter_roots()?;

    // Bind setup input devs before.
    for dev in &setup.input_devs {
        inner_bind_device(dev)?;
    }

    Ok(())
}

fn write_uid_map(uid: nix::unistd::Uid, gid: nix::unistd::Gid) -> anyhow::Result<()> {
    let uid_map_content = format!("{} {} 1\n", uid.as_raw(), uid.as_raw());
    let gid_map_content = format!("{} {} 1\n", gid.as_raw(), gid.as_raw());

    File::create("/proc/self/setgroups")
        .context("Failed to open setgroups file")?
        .write_all("deny\n".as_bytes())
        .context("Failed to write to setgroups file")?;

    File::create("/proc/self/uid_map")
        .context("Failed to open uid_map file")?
        .write_all(uid_map_content.as_bytes())
        .context("Failed to write to uid_map file")?;
    File::create("/proc/self/gid_map")
        .context("Failed to open gid_map file")?
        .write_all(gid_map_content.as_bytes())
        .context("Failed to write to gid_map file")?;

    Ok(())
}

const NONE_STR: Option<&OsStr> = None::<&OsStr>;

fn setup_roots() -> anyhow::Result<()> {
    // Mount to just make sure we are in a valid state (not needed but whatever, maybe also makes changes in this mode (tmpfs) not reflect upwards or into the container)
    mount(
        NONE_STR,
        "/",
        NONE_STR,
        MsFlags::MS_REC | MsFlags::MS_SILENT | MsFlags::MS_SLAVE,
        None::<&OsStr>,
    ).context("Failed to remount host root")?;

    // Normalize new tmpfs.
    mount(
        NONE_STR,
        "/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to mount tmpfs on /tmp")?;

    // Setup temp files to be filled later :D
    mkdir("/tmp/newroot", Mode::from_bits_retain(0o755)).context("Failed to create /tmp/newroot directory")?;
    mkdir("/tmp/oldroot", Mode::from_bits_retain(0o755)).context("Failed to create /tmp/oldroot directory")?;

    // Make newroot a real mount point (may not be needed)
    mount(
        Some("/tmp/newroot"),
        "/tmp/newroot",
        NONE_STR,
        MsFlags::MS_MGC_VAL | MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_SILENT,
        None::<&OsStr>,
    ).context("Failed to mount /tmp/newroot")?;

    // Populates /oldroot (as we are now on /tmp) At this point, we are on a "clean" system on tmpfs.
    pivot_root("/tmp", "/tmp/oldroot").context("Failed to pivot_root to /tmp/oldroot")?;
    

    Ok(())
}

fn cleanup_enter_roots() -> anyhow::Result<()> {
    // No clue.. Permissions reallocation? (may not be needed)
    // Drops SETUID on new root, not sure if needed, but why not...
    mount(
        NONE_STR,
        "/newroot",
        NONE_STR,
        MsFlags::MS_NOSUID
            | MsFlags::MS_REMOUNT
            | MsFlags::MS_BIND
            | MsFlags::MS_SILENT
            | MsFlags::MS_RELATIME,
        None::<&OsStr>,
    ).context("Failed to remount /newroot for permissions")?;

    // Maybe set oldroot to private so unmounting doesnt affect anything? Not sure here
    mount(
        Some("/oldroot"),
        "/oldroot",
        NONE_STR,
        MsFlags::MS_REC | MsFlags::MS_SILENT | MsFlags::MS_PRIVATE,
        None::<&OsStr>,
    ).context("Failed to set /oldroot as private")?;

    // Remove old root
    umount2("/oldroot", MntFlags::MNT_DETACH).context("Failed to unmount /oldroot")?;

    // WOAH can you do this??? Stores a FD outside BEFORE we enter the pivot root
    let old_root = openat(
        AT_FDCWD,
        "/",
        OFlag::O_RDONLY | OFlag::O_DIRECTORY,
        Mode::empty(),
    ).context("Failed to open / for dirfd before pivot")?;

    // Switch root to new root.
    chdir("/newroot").context("Failed to chdir to /newroot")?;
    pivot_root(".", ".").context("Failed to pivot_root to new root")?;

    // Clean up after ourselves, removing the old root before we pivoted (works by black magic) [escapes the pivot root]
    fchdir(old_root).context("Failed to fchdir to old_root")?;
    umount2(".", MntFlags::MNT_DETACH).context("Failed to unmount virtual workspace")?;

    // Go back into the pivot_root.
    chdir("/").context("Failed to chdir back to /")?;

    // Drop our permissions to the normal user's
    umask(Mode::from_bits_retain(0o022));

    Ok(())
}

// Copied below from rust stdlib, because they are not public (For some reason) and I didnt want to std::transmute.

fn cvt_libc_err(t: i64) -> std::io::Result<i64> {
    if t == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

pub struct PidFD {
    fd: OwnedFd,
    last_exit_code: Option<i32>,
}

impl PidFD {
    pub fn open(pid: u32) -> anyhow::Result<Self> {
        unsafe {
            // Please note, nix has nothing for pidfd for whatever reason, I had to clone from cmd.spawn() [process.spawn] and they just use libc too lol.
            // Race condition here may be possible before pidfd creation, but because we never called `wait`, kernel should make sure the PID is still valid here.
            let pidfd_raw = cvt_libc_err(libc::syscall(libc::SYS_pidfd_open, pid, 0))
                .context("Failed to open pidfd")?;

            Ok(Self {
                fd: OwnedFd::from_raw_fd(pidfd_raw as RawFd),
                last_exit_code: None,
            })
        }
    }

    pub fn open_from_fd(fd: &OwnedFd) -> anyhow::Result<Self> {
        Ok(Self {
            fd: fd.try_clone()?,
            last_exit_code: None,
        })
    }

    pub fn signal(&mut self, signal: signal::Signal) -> anyhow::Result<()> {
        cvt_libc_err(unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd() as u32,
                signal,
                core::ptr::null::<()>(),
                0,
            )
        }).context("Failed to send signal to pidfd")
        .map(std::mem::drop)
    }

    // Note new kernels only. 6.15+ as said from stdlib, but this is handled safely in wait_inner
    fn recover_reaped_exit_code(&mut self) -> anyhow::Result<i32> {
        let mut pidfd_info: libc::pidfd_info = unsafe { core::mem::zeroed() };
        pidfd_info.mask = libc::PIDFD_INFO_EXIT as u64;
        cvt_libc_err(unsafe {
            libc::ioctl(self.fd.as_raw_fd(), libc::PIDFD_GET_INFO, &mut pidfd_info).into()
        }).context("Failed to recover exit code from pidfd")?;

        self.last_exit_code = Some(pidfd_info.exit_code);
        Ok(pidfd_info.exit_code)
    }

    // pub fn try_wait(&mut self) -> anyhow::Result<Option<i32>> {
    //     self.wait_inner(libc::WEXITED | libc::WNOHANG)
    // }
    // pub fn wait(&mut self) -> anyhow::Result<Option<i32>> {
    //     self.wait_inner(libc::WEXITED)
    // }
    // fn wait_inner(&mut self, options: i32) -> anyhow::Result<Option<i32>> {
    //     // libc::WEXITED | libc::WNOHANG
    //     if self.last_exit_code.is_some() {
    //         return Ok(self.last_exit_code);
    //     };

    //     let mut siginfo: libc::siginfo_t = unsafe { core::mem::zeroed() };
    //     let r = cvt_libc_err(unsafe {
    //         libc::waitid(
    //             libc::P_PIDFD,
    //             self.fd.as_raw_fd() as u32,
    //             &mut siginfo,
    //             options,
    //         )
    //         .into()
    //     });

    //     match r {
    //         Err(waitid_err) if waitid_err.raw_os_error() == Some(libc::ECHILD) => {
    //             // already reaped
    //             match self.recover_reaped_exit_code() {
    //                 Ok(exit_status) => {
    //                     self.last_exit_code = Some(exit_status);
    //                     return Ok(self.last_exit_code);
    //                 }
    //                 Err(_) => return Err(anyhow::Error::from(waitid_err)),
    //             }
    //         }
    //         Err(e) => return Err(anyhow::Error::from(e)),
    //         Ok(_) => {}
    //     }

    //     if unsafe { siginfo.si_pid() } == 0 {
    //         self.last_exit_code = None;
    //     } else {
    //         self.last_exit_code = Some(Self::from_waitid_siginfo(siginfo));
    //     }
    //     Ok(self.last_exit_code)
    // }

    pub fn wait(&mut self) -> anyhow::Result<Option<i32>> {
        self.wait_inner(false)
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<i32>> {
        self.wait_inner(true)
    }

    fn wait_inner(&mut self, nonblocking: bool) -> anyhow::Result<Option<i32>> { // TODO WARNING THIS IS USING NEW LINUX KERNEL CALL AS A REQUIREMENT. SHOULD INSTAED CALL TO THE IPC TO GET THIS INFO FROM REAL CHILD TO CHECK IF DEAD.
        if self.last_exit_code.is_some() {
            return Ok(self.last_exit_code);
        }

        // Poll the pidfd for readability (indicates process has exited)
        let mut pollfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let timeout = if nonblocking { 0 } else { -1 };
        let poll_result = unsafe { libc::poll(&mut pollfd, 1, timeout) };

        match cvt_libc_err(poll_result as i64) {
            Err(e) => return Err(anyhow::Error::from(e)),
            Ok(0) => {
                // Timeout (only in nonblocking mode)
                return Ok(None);
            }
            Ok(_) => {} // pidfd is readable, process exited
        }

        // Process has exited; try to get exit code
        match self.recover_reaped_exit_code() {
            Ok(exit_code) => {
                self.last_exit_code = Some(exit_code);
                Ok(self.last_exit_code)
            }
            Err(e) => {
                // Fallback: on older kernels, we can't recover exit code
                // but we know the process exited
                eprintln!("Warning: could not recover exit code: {}", e);
                self.last_exit_code = Some(0);
                Ok(self.last_exit_code)
            }
        }
    }


    fn from_waitid_siginfo(siginfo: libc::siginfo_t) -> i32 {
        let status = unsafe { siginfo.si_status() };

        match siginfo.si_code {
            libc::CLD_EXITED => (status & 0xff) << 8,
            libc::CLD_KILLED => status,
            libc::CLD_DUMPED => status | 0x80,
            libc::CLD_CONTINUED => 0xffff,
            libc::CLD_STOPPED | libc::CLD_TRAPPED => ((status & 0xff) << 8) | 0x7f,
            _ => unreachable!("waitid() should only return the above codes"),
        }
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn as_owned(&self) -> Result<OwnedFd, std::io::Error> {
        self.fd.try_clone()
    }
}
