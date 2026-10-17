//! C ABI for Go discovery (cgo).

use crate::dump::{DumpIndex, DumpRow};
use crate::fetch::{self, CatalogPage};
use crate::search::SearchIndex;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, RwLock};

static DUMP: Mutex<Option<Arc<DumpIndex>>> = Mutex::new(None);

#[repr(C)]
pub struct WebtorDumpRow {
    pub name: [c_char; 512],
    pub size: i64,
    pub seeders: i32,
    pub leechers: i32,
}

fn fill_row(dst: &mut WebtorDumpRow, row: &DumpRow) {
    dst.size = row.size;
    dst.seeders = row.seeders;
    dst.leechers = row.leechers;
    dst.name = [0; 512];
    let bytes = row.name.as_bytes();
    let mut n = bytes.len().min(511);
    while !row.name.is_char_boundary(n) {
        n -= 1;
    }
    for (i, &b) in bytes[..n].iter().enumerate() {
        dst.name[i] = b as c_char;
    }
}

fn cstr_to_path(path: *const c_char) -> Result<std::path::PathBuf, ()> {
    if path.is_null() {
        return Err(());
    }
    let s = unsafe { CStr::from_ptr(path) }.to_str().map_err(|_| ())?;
    Ok(Path::new(s).to_path_buf())
}

fn dump_arc() -> Option<Arc<DumpIndex>> {
    DUMP.lock().ok().and_then(|g| g.clone())
}

/// Load (or reload) the mmap dump index. Returns row count, or -1 on error.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_load(path: *const c_char) -> c_int {
    let path = match cstr_to_path(path) {
        Ok(p) => p,
        Err(()) => return -1,
    };
    match DumpIndex::open(&path) {
        Ok(idx) => {
            let n = idx.len() as c_int;
            if let Ok(mut g) = DUMP.lock() {
                *g = Some(Arc::new(idx));
                n
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

/// Drop the loaded dump index.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_free() {
    if let Ok(mut g) = DUMP.lock() {
        *g = None;
    }
}

#[no_mangle]
pub unsafe extern "C" fn webtor_dump_len() -> c_int {
    dump_arc().map(|i| i.len() as c_int).unwrap_or(0)
}

/// Lookup lowercase 40-hex infohash. Returns 1 hit, 0 miss, -1 error.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_lookup(hash: *const c_char, out: *mut WebtorDumpRow) -> c_int {
    if hash.is_null() || out.is_null() {
        return -1;
    }
    let hash = match CStr::from_ptr(hash).to_str() {
        Ok(s) => s.as_bytes(),
        Err(_) => return -1,
    };
    if hash.len() != 40 {
        return -1;
    }
    let mut lower = [0u8; 40];
    for (i, &b) in hash.iter().enumerate() {
        lower[i] = b.to_ascii_lowercase();
    }
    let Some(idx) = dump_arc() else {
        return -1;
    };
    match idx.lookup(&lower) {
        Some(row) => {
            fill_row(&mut *out, &row);
            1
        }
        None => 0,
    }
}

pub type WebtorDumpIter = Option<
    unsafe extern "C" fn(hash: *const c_char, row: *const WebtorDumpRow, ctx: *mut c_void) -> c_int,
>;

/// Iterate rows with seeders > 0. Lock is released before callbacks so Go can
/// look up / apply without deadlocking the dump mutex.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_for_each_seeded(
    cb: WebtorDumpIter,
    ctx: *mut c_void,
) -> c_int {
    let Some(cb) = cb else {
        return -1;
    };
    let Some(idx) = dump_arc() else {
        return -1;
    };
    let mut count = 0i32;
    let mut stop = false;
    idx.for_each(|hash, row| {
        if stop {
            return false;
        }
        if row.seeders <= 0 {
            return true;
        }
        let mut c_row = WebtorDumpRow {
            name: [0; 512],
            size: 0,
            seeders: 0,
            leechers: 0,
        };
        fill_row(&mut c_row, &row);
        let mut hash_buf = [0u8; 41];
        hash_buf[..40].copy_from_slice(hash);
        let hash_c = hash_buf.as_ptr() as *const c_char;
        let cont = cb(hash_c, &c_row, ctx);
        count += 1;
        if cont == 0 {
            stop = true;
            return false;
        }
        true
    });
    count
}

/// Download dump if missing/stale. force!=0 always downloads. 0 ok, -1 fail.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_ensure(path: *const c_char, force: c_int) -> c_int {
    let path = match cstr_to_path(path) {
        Ok(p) => p,
        Err(()) => return -1,
    };
    match fetch::ensure_dump(&path, force != 0) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn webtor_catalog_fetch(
    query: *const c_char,
    size: c_int,
    after: c_int,
    json_out: *mut *mut c_char,
    next_out: *mut c_int,
) -> c_int {
    if query.is_null() || json_out.is_null() {
        return -1;
    }
    *json_out = ptr::null_mut();
    if !next_out.is_null() {
        *next_out = -1;
    }
    let q = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let page = match fetch::fetch_catalog_page(q, size, after) {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if let Some(n) = page.next {
        if !next_out.is_null() {
            *next_out = n;
        }
    }
    let json = catalog_to_json(&page);
    match CString::new(json) {
        Ok(cs) => {
            let n = page.torrents.len() as c_int;
            *json_out = cs.into_raw();
            n
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn webtor_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    drop(CString::from_raw(s));
}

fn catalog_to_json(page: &CatalogPage) -> String {
    let mut out = String::from("[");
    for (i, t) in page.torrents.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"infohash\":\"{}\",\"name\":{},\"size_bytes\":{},\"seeders\":{},\"leechers\":{}}}",
            t.infohash,
            json_escape(&t.name),
            t.size_bytes,
            t.seeders,
            t.leechers
        ));
    }
    out.push(']');
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Search handles are independent per dashboard (including Go tests).
#[no_mangle]
pub extern "C" fn webtor_search_new() -> *mut c_void {
    Box::into_raw(Box::new(RwLock::new(SearchIndex::default()))) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn webtor_search_free(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle as *mut RwLock<SearchIndex>));
    }
}
#[no_mangle]
pub unsafe extern "C" fn webtor_search_add(
    handle: *mut c_void,
    data: *const u8,
    length: usize,
) -> c_int {
    if handle.is_null() || data.is_null() || length > 8 * 1024 * 1024 {
        return -1;
    }
    let index = &*(handle as *const RwLock<SearchIndex>);
    match index.write() {
        Ok(mut index) => index.add_batch(std::slice::from_raw_parts(data, length)) as c_int,
        Err(_) => -1,
    }
}
/// Return total matches; copy at most capacity packed 20-byte hashes.
#[no_mangle]
pub unsafe extern "C" fn webtor_search_query(
    handle: *mut c_void,
    query: *const c_char,
    out: *mut u8,
    capacity: usize,
) -> c_int {
    if handle.is_null() || query.is_null() || (capacity > 0 && out.is_null()) {
        return -1;
    }
    let Ok(query) = CStr::from_ptr(query).to_str() else {
        return -1;
    };
    let index = &*(handle as *const RwLock<SearchIndex>);
    let Ok(index) = index.read() else {
        return -1;
    };
    let Some(matches) = index.candidates(query) else {
        return -1;
    };
    for (i, hash) in matches.iter().take(capacity).enumerate() {
        ptr::copy_nonoverlapping(hash.as_ptr(), out.add(i * 20), 20);
    }
    matches.len() as c_int
}

/// One call for all pending hashes. Workers only read the mmap; callbacks run
/// sequentially after joining so Go owns all observation mutation.
#[no_mangle]
pub unsafe extern "C" fn webtor_dump_lookup_many(
    hashes: *const u8,
    count: usize,
    cb: WebtorDumpIter,
    ctx: *mut c_void,
) -> c_int {
    if hashes.is_null() || count > 100_000 {
        return -1;
    }
    let Some(cb) = cb else {
        return -1;
    };
    let Some(index) = dump_arc() else {
        return -1;
    };
    let input = std::slice::from_raw_parts(hashes, count * 40);
    let workers = if count >= 4096 {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
    } else {
        1
    };
    let chunk_bytes = count.div_ceil(workers).max(1) * 40;
    let hits = std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for chunk in input.chunks(chunk_bytes) {
            let index = &index;
            jobs.push(scope.spawn(move || {
                chunk
                    .chunks_exact(40)
                    .filter_map(|hash| {
                        index.lookup(hash).map(|row| {
                            let mut key = [0u8; 41];
                            key[..40].copy_from_slice(hash);
                            (key, row)
                        })
                    })
                    .collect::<Vec<_>>()
            }));
        }
        jobs.into_iter()
            .flat_map(|job| job.join().unwrap_or_default())
            .collect::<Vec<_>>()
    });
    let mut applied = 0;
    for (hash, row) in hits {
        let mut out = WebtorDumpRow {
            name: [0; 512],
            size: 0,
            seeders: 0,
            leechers: 0,
        };
        fill_row(&mut out, &row);
        applied += 1;
        if cb(hash.as_ptr() as *const c_char, &out, ctx) == 0 {
            break;
        }
    }
    applied
}
