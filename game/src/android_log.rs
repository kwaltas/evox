//! Android plumbing: send `stdout`/`stderr` to logcat (tag `EVOX`) so the
//! engine's loud diagnostics are visible, and read QA arguments.

use std::ffi::{c_char, CString};
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
}

/// Route fds 1 and 2 through a pipe whose reader forwards each line to logcat.
pub fn redirect_stdio() {
    let mut fds = [0i32; 2];
    // SAFETY: plain POSIX calls; the read end is owned by the forwarder thread.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
        let reader = std::fs::File::from_raw_fd(fds[0]);
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if let Ok(c) = CString::new(line) {
                    __android_log_write(4, c"EVOX".as_ptr(), c.as_ptr());
                }
            }
        });
    }
}
