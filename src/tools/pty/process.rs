use std::sync::Mutex;

use portable_pty::Child;

use super::types::PtySessionStatus;

pub(crate) fn kill_pty_child(
    child: &mut dyn Child,
    child_pid: Option<u32>,
) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    if let Some(child_pid) = child_pid {
        let process_group_result = kill_child_process_group(child_pid);
        let child_result = child.kill();
        return match (process_group_result, child_result) {
            (Err(group_err), _) => Err(group_err),
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(err)) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Ok(()), Err(err)) => Err(err),
        };
    }

    #[cfg(not(unix))]
    let _ = child_pid;

    child.kill()
}

pub(crate) fn restore_running_after_failed_kill(status: &Mutex<PtySessionStatus>) {
    let mut status = status.lock().expect("pty status lock");
    if matches!(*status, PtySessionStatus::Killing) {
        *status = PtySessionStatus::Running;
    }
}

#[cfg(unix)]
fn kill_child_process_group(child_pid: u32) -> Result<(), std::io::Error> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::{Pid, getpgid};

    let child_pid = Pid::from_raw(child_pid as i32);
    let process_group_id = match getpgid(Some(child_pid)) {
        Ok(process_group_id) => process_group_id,
        Err(Errno::ESRCH) => return Ok(()),
        Err(err) => return Err(std::io::Error::from_raw_os_error(err as i32)),
    };

    match killpg(process_group_id, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(std::io::Error::from_raw_os_error(err as i32)),
    }
}
