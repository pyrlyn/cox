//! Minimal direct binding over `libsqlite3-sys`: no ORM, no extra layers.

use std::ffi::{CStr, CString, c_int};
use std::ptr;

use libsqlite3_sys as ffi;

use crate::{Error, Result};

pub struct Db {
    raw: *mut ffi::sqlite3,
}

// SAFETY: SQLite is built in serialized mode by `bundled`; each `Db` is used by
// one thread at a time (callers wrap it in a mutex).
unsafe impl Send for Db {}

fn errmsg(raw: *mut ffi::sqlite3) -> Error {
    // SAFETY: `raw` is a live connection handle.
    let s = unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(raw)) };
    Error(format!("sqlite: {}", s.to_string_lossy()))
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let c = CString::new(path).map_err(|e| Error(e.to_string()))?;
        let mut raw = ptr::null_mut();
        let flags = ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_NOMUTEX;
        // SAFETY: valid C string and out pointer.
        let rc = unsafe { ffi::sqlite3_open_v2(c.as_ptr(), &mut raw, flags, ptr::null()) };
        if rc != ffi::SQLITE_OK {
            return Err(errmsg(raw));
        }
        Ok(Self { raw })
    }

    pub fn exec(&self, sql: &str) -> Result<()> {
        let c = CString::new(sql).map_err(|e| Error(e.to_string()))?;
        // SAFETY: live handle, valid SQL string.
        let rc = unsafe { ffi::sqlite3_exec(self.raw, c.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
        if rc == ffi::SQLITE_OK { Ok(()) } else { Err(errmsg(self.raw)) }
    }

    pub fn prepare(&self, sql: &str) -> Result<Stmt<'_>> {
        let c = CString::new(sql).map_err(|e| Error(e.to_string()))?;
        let mut st = ptr::null_mut();
        // SAFETY: live handle, valid SQL string.
        let rc = unsafe { ffi::sqlite3_prepare_v2(self.raw, c.as_ptr(), -1, &mut st, ptr::null_mut()) };
        if rc != ffi::SQLITE_OK {
            return Err(errmsg(self.raw));
        }
        Ok(Stmt { db: self, raw: st })
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        // SAFETY: statements borrow `Db`, so all are finalized by now.
        unsafe { ffi::sqlite3_close(self.raw) };
    }
}

pub struct Stmt<'a> {
    db: &'a Db,
    raw: *mut ffi::sqlite3_stmt,
}

impl Stmt<'_> {
    pub fn bind_i64(&mut self, i: c_int, v: i64) -> Result<()> {
        // SAFETY: live statement.
        self.ok(unsafe { ffi::sqlite3_bind_int64(self.raw, i, v) })
    }

    pub fn bind_blob(&mut self, i: c_int, v: &[u8]) -> Result<()> {
        // SAFETY: SQLITE_TRANSIENT makes SQLite copy the bytes.
        self.ok(unsafe {
            ffi::sqlite3_bind_blob(self.raw, i, v.as_ptr().cast(), v.len() as c_int, ffi::SQLITE_TRANSIENT())
        })
    }

    fn ok(&self, rc: c_int) -> Result<()> {
        if rc == ffi::SQLITE_OK { Ok(()) } else { Err(errmsg(self.db.raw)) }
    }

    /// Steps once; `true` when a row is available.
    pub fn step(&mut self) -> Result<bool> {
        // SAFETY: live statement.
        match unsafe { ffi::sqlite3_step(self.raw) } {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => Ok(false),
            _ => Err(errmsg(self.db.raw)),
        }
    }

    pub fn reset(&mut self) {
        // SAFETY: live statement.
        unsafe { ffi::sqlite3_reset(self.raw) };
    }

    pub fn col_i64(&self, i: c_int) -> i64 {
        // SAFETY: called while positioned on a row.
        unsafe { ffi::sqlite3_column_int64(self.raw, i) }
    }

    /// Borrowed view of a BLOB column, valid until the next step/reset.
    pub fn col_blob(&self, i: c_int) -> &[u8] {
        // SAFETY: pointer/length pair returned by SQLite for the current row.
        unsafe {
            let p = ffi::sqlite3_column_blob(self.raw, i);
            let n = ffi::sqlite3_column_bytes(self.raw, i) as usize;
            if p.is_null() { &[] } else { std::slice::from_raw_parts(p.cast(), n) }
        }
    }
}

impl Drop for Stmt<'_> {
    fn drop(&mut self) {
        // SAFETY: live statement, finalized once.
        unsafe { ffi::sqlite3_finalize(self.raw) };
    }
}
