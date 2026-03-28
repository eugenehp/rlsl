//! `lsl-sys` — C ABI shared library that is a drop-in replacement for `liblsl`.
//!
//! Every `#[no_mangle] pub extern "C"` function in this crate is exported from
//! the resulting `liblsl.{dylib,so,dll}`.
#![allow(unused_variables)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::explicit_counter_loop)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_float, c_ulong, c_void};
use std::ptr;

use lsl_core::clock::local_clock;
use lsl_core::inlet::StreamInlet;
use lsl_core::outlet::StreamOutlet;
use lsl_core::resolver::{self, ContinuousResolver};
use lsl_core::stream_info::StreamInfo;
use lsl_core::types::*;
use lsl_core::xml_dom::XmlNode;

// === Opaque handle types ===

pub struct LslStreamInfo {
    pub info: StreamInfo,
}
pub struct LslOutlet {
    pub outlet: StreamOutlet,
}
pub struct LslInlet {
    pub inlet: StreamInlet,
}
pub struct LslContinuousResolver {
    pub resolver: ContinuousResolver,
}
pub struct LslXmlPtr {
    pub node: XmlNode,
}

// === Helper macros ===

macro_rules! null_check {
    ($ptr:expr) => {
        if $ptr.is_null() {
            return ptr::null_mut();
        }
    };
    ($ptr:expr, $default:expr) => {
        if $ptr.is_null() {
            return $default;
        }
    };
}

fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .unwrap_or("")
        .to_string()
}

fn string_to_cstr(s: &str) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

// === common.h ===

#[no_mangle]
pub extern "C" fn lsl_protocol_version() -> i32 {
    LSL_PROTOCOL_VERSION
}

#[no_mangle]
pub extern "C" fn lsl_library_version() -> i32 {
    LSL_LIBRARY_VERSION
}

#[no_mangle]
pub extern "C" fn lsl_library_info() -> *const c_char {
    static INFO: &[u8] = b"lsl-rs 0.1.0\0";
    INFO.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn lsl_local_clock() -> c_double {
    local_clock()
}

#[no_mangle]
pub extern "C" fn lsl_destroy_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[no_mangle]
pub extern "C" fn lsl_last_error() -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    EMPTY.as_ptr() as *const c_char
}

// === streaminfo.h ===

#[no_mangle]
pub extern "C" fn lsl_create_streaminfo(
    name: *const c_char,
    type_: *const c_char,
    channel_count: i32,
    nominal_srate: c_double,
    channel_format: i32,
    source_id: *const c_char,
) -> *mut LslStreamInfo {
    let name = cstr_to_string(name);
    let type_ = cstr_to_string(type_);
    let source_id = cstr_to_string(source_id);
    let fmt = ChannelFormat::from_i32(channel_format);

    let info = StreamInfo::new(
        &name,
        &type_,
        channel_count as u32,
        nominal_srate,
        fmt,
        &source_id,
    );
    Box::into_raw(Box::new(LslStreamInfo { info }))
}

#[no_mangle]
pub extern "C" fn lsl_destroy_streaminfo(info: *mut LslStreamInfo) {
    if !info.is_null() {
        unsafe {
            let _ = Box::from_raw(info);
        }
    }
}

#[no_mangle]
pub extern "C" fn lsl_copy_streaminfo(info: *mut LslStreamInfo) -> *mut LslStreamInfo {
    null_check!(info);
    let src = unsafe { &*info };
    // Deep copy: create a fresh StreamInfo with the same data
    let s = &src.info;
    let new_info = StreamInfo::new(
        &s.name(),
        &s.type_(),
        s.channel_count(),
        s.nominal_srate(),
        s.channel_format(),
        &s.source_id(),
    );
    new_info.set_uid(&s.uid());
    new_info.set_created_at(s.created_at());
    new_info.set_session_id(&s.session_id());
    new_info.set_hostname(&s.hostname());
    new_info.set_v4address(&s.v4address());
    new_info.set_v4data_port(s.v4data_port());
    new_info.set_v4service_port(s.v4service_port());
    new_info.set_v6address(&s.v6address());
    new_info.set_v6data_port(s.v6data_port());
    new_info.set_v6service_port(s.v6service_port());
    new_info.set_version(s.version());
    Box::into_raw(Box::new(LslStreamInfo { info: new_info }))
}

#[no_mangle]
pub extern "C" fn lsl_get_name(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    let info = unsafe { &*info };
    string_to_cstr(&info.info.name())
}

#[no_mangle]
pub extern "C" fn lsl_get_type(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    string_to_cstr(&unsafe { &*info }.info.type_())
}

#[no_mangle]
pub extern "C" fn lsl_get_channel_count(info: *mut LslStreamInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { &*info }.info.channel_count() as i32
}

#[no_mangle]
pub extern "C" fn lsl_get_nominal_srate(info: *mut LslStreamInfo) -> c_double {
    if info.is_null() {
        return 0.0;
    }
    unsafe { &*info }.info.nominal_srate()
}

#[no_mangle]
pub extern "C" fn lsl_get_channel_format(info: *mut LslStreamInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { &*info }.info.channel_format() as i32
}

#[no_mangle]
pub extern "C" fn lsl_get_source_id(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    string_to_cstr(&unsafe { &*info }.info.source_id())
}

#[no_mangle]
pub extern "C" fn lsl_get_version(info: *mut LslStreamInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { &*info }.info.version()
}

#[no_mangle]
pub extern "C" fn lsl_get_created_at(info: *mut LslStreamInfo) -> c_double {
    if info.is_null() {
        return 0.0;
    }
    unsafe { &*info }.info.created_at()
}

#[no_mangle]
pub extern "C" fn lsl_get_uid(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    string_to_cstr(&unsafe { &*info }.info.uid())
}

#[no_mangle]
pub extern "C" fn lsl_get_session_id(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    string_to_cstr(&unsafe { &*info }.info.session_id())
}

#[no_mangle]
pub extern "C" fn lsl_get_hostname(info: *mut LslStreamInfo) -> *const c_char {
    null_check!(info, ptr::null());
    string_to_cstr(&unsafe { &*info }.info.hostname())
}

#[no_mangle]
pub extern "C" fn lsl_get_channel_bytes(info: *mut LslStreamInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { &*info }.info.channel_bytes() as i32
}

#[no_mangle]
pub extern "C" fn lsl_get_sample_bytes(info: *mut LslStreamInfo) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { &*info }.info.sample_bytes() as i32
}

#[no_mangle]
pub extern "C" fn lsl_get_xml(info: *mut LslStreamInfo) -> *mut c_char {
    null_check!(info);
    let xml = unsafe { &*info }.info.to_fullinfo_message();
    string_to_cstr(&xml)
}

#[no_mangle]
pub extern "C" fn lsl_get_desc(info: *mut LslStreamInfo) -> *mut LslXmlPtr {
    null_check!(info);
    let node = unsafe { &*info }.info.desc();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_streaminfo_from_xml(xml: *const c_char) -> *mut LslStreamInfo {
    let xml = cstr_to_string(xml);
    match StreamInfo::from_shortinfo_message(&xml) {
        Some(info) => Box::into_raw(Box::new(LslStreamInfo { info })),
        None => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn lsl_stream_info_matches_query(
    info: *mut LslStreamInfo,
    query: *const c_char,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    let query = cstr_to_string(query);
    unsafe { &*info }.info.matches_query(&query) as i32
}

// === outlet.h ===

#[no_mangle]
pub extern "C" fn lsl_create_outlet(
    info: *mut LslStreamInfo,
    chunk_size: i32,
    max_buffered: i32,
) -> *mut LslOutlet {
    null_check!(info);
    let info = unsafe { &*info };
    let outlet = StreamOutlet::new(&info.info, chunk_size, max_buffered);
    Box::into_raw(Box::new(LslOutlet { outlet }))
}

#[no_mangle]
pub extern "C" fn lsl_create_outlet_ex(
    info: *mut LslStreamInfo,
    chunk_size: i32,
    max_buffered: i32,
    _flags: i32,
) -> *mut LslOutlet {
    lsl_create_outlet(info, chunk_size, max_buffered)
}

#[no_mangle]
pub extern "C" fn lsl_destroy_outlet(out: *mut LslOutlet) {
    if !out.is_null() {
        unsafe {
            let _ = Box::from_raw(out);
        }
    }
}

#[no_mangle]
pub extern "C" fn lsl_get_info(out: *mut LslOutlet) -> *mut LslStreamInfo {
    null_check!(out);
    let info = unsafe { &*out }.outlet.info().clone();
    Box::into_raw(Box::new(LslStreamInfo { info }))
}

macro_rules! impl_push_sample {
    ($fname:ident, $fnamert:ident, $fnamertp:ident, $ty:ty, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fname(out: *mut LslOutlet, data: *const $ty) -> i32 {
            $fnamertp(out, data, 0.0, 1)
        }
        #[no_mangle]
        pub extern "C" fn $fnamert(
            out: *mut LslOutlet,
            data: *const $ty,
            timestamp: c_double,
        ) -> i32 {
            $fnamertp(out, data, timestamp, 1)
        }
        #[no_mangle]
        pub extern "C" fn $fnamertp(
            out: *mut LslOutlet,
            data: *const $ty,
            timestamp: c_double,
            pushthrough: i32,
        ) -> i32 {
            if out.is_null() || data.is_null() {
                return -3;
            }
            let outlet = unsafe { &*out };
            let nch = outlet.outlet.info().channel_count() as usize;
            let slice = unsafe { std::slice::from_raw_parts(data, nch) };
            outlet.outlet.$method(slice, timestamp, pushthrough != 0);
            0
        }
    };
}

impl_push_sample!(
    lsl_push_sample_f,
    lsl_push_sample_ft,
    lsl_push_sample_ftp,
    c_float,
    push_sample_f
);
impl_push_sample!(
    lsl_push_sample_d,
    lsl_push_sample_dt,
    lsl_push_sample_dtp,
    c_double,
    push_sample_d
);
impl_push_sample!(
    lsl_push_sample_i,
    lsl_push_sample_it,
    lsl_push_sample_itp,
    i32,
    push_sample_i32
);
impl_push_sample!(
    lsl_push_sample_s,
    lsl_push_sample_st,
    lsl_push_sample_stp,
    i16,
    push_sample_i16
);
impl_push_sample!(
    lsl_push_sample_l,
    lsl_push_sample_lt,
    lsl_push_sample_ltp,
    i64,
    push_sample_i64
);

#[no_mangle]
pub extern "C" fn lsl_push_sample_c(out: *mut LslOutlet, data: *const c_char) -> i32 {
    lsl_push_sample_ctp(out, data, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_ct(
    out: *mut LslOutlet,
    data: *const c_char,
    timestamp: c_double,
) -> i32 {
    lsl_push_sample_ctp(out, data, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_ctp(
    out: *mut LslOutlet,
    data: *const c_char,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    let slice = unsafe { std::slice::from_raw_parts(data as *const i8, nch) };
    outlet.outlet.push_sample_i16(
        &slice.iter().map(|&v| v as i16).collect::<Vec<_>>(),
        timestamp,
        pushthrough != 0,
    );
    0
}

#[no_mangle]
pub extern "C" fn lsl_push_sample_str(out: *mut LslOutlet, data: *const *const c_char) -> i32 {
    lsl_push_sample_strtp(out, data, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_strt(
    out: *mut LslOutlet,
    data: *const *const c_char,
    timestamp: c_double,
) -> i32 {
    lsl_push_sample_strtp(out, data, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_strtp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    let ptrs = unsafe { std::slice::from_raw_parts(data, nch) };
    let strings: Vec<String> = ptrs.iter().map(|&p| cstr_to_string(p)).collect();
    outlet
        .outlet
        .push_sample_str(&strings, timestamp, pushthrough != 0);
    0
}

#[no_mangle]
pub extern "C" fn lsl_push_sample_v(out: *mut LslOutlet, data: *const c_void) -> i32 {
    lsl_push_sample_vtp(out, data, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_vt(
    out: *mut LslOutlet,
    data: *const c_void,
    timestamp: c_double,
) -> i32 {
    lsl_push_sample_vtp(out, data, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_vtp(
    out: *mut LslOutlet,
    data: *const c_void,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nbytes = outlet.outlet.info().sample_bytes();
    let slice = unsafe { std::slice::from_raw_parts(data as *const u8, nbytes) };
    outlet
        .outlet
        .push_sample_raw(slice, timestamp, pushthrough != 0);
    0
}

#[no_mangle]
pub extern "C" fn lsl_push_sample_buf(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
) -> i32 {
    lsl_push_sample_buftp(out, data, lengths, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_buft(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    timestamp: c_double,
) -> i32 {
    lsl_push_sample_buftp(out, data, lengths, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_sample_buftp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() || lengths.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    let ptrs = unsafe { std::slice::from_raw_parts(data, nch) };
    let lens = unsafe { std::slice::from_raw_parts(lengths, nch) };
    let strings: Vec<String> = ptrs
        .iter()
        .zip(lens)
        .map(|(&p, &l)| {
            let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, l as usize) };
            String::from_utf8_lossy(bytes).into_owned()
        })
        .collect();
    outlet
        .outlet
        .push_sample_str(&strings, timestamp, pushthrough != 0);
    0
}

// === Push chunk functions ===

macro_rules! impl_push_chunk {
    ($fname:ident, $fnamet:ident, $fnametp:ident, $fnametn:ident, $fnametnp:ident, $ty:ty, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fname(
            out: *mut LslOutlet,
            data: *const $ty,
            data_elements: c_ulong,
        ) -> i32 {
            $fnametp(out, data, data_elements, 0.0, 1)
        }
        #[no_mangle]
        pub extern "C" fn $fnamet(
            out: *mut LslOutlet,
            data: *const $ty,
            data_elements: c_ulong,
            timestamp: c_double,
        ) -> i32 {
            $fnametp(out, data, data_elements, timestamp, 1)
        }
        #[no_mangle]
        pub extern "C" fn $fnametp(
            out: *mut LslOutlet,
            data: *const $ty,
            data_elements: c_ulong,
            timestamp: c_double,
            pushthrough: i32,
        ) -> i32 {
            if out.is_null() || data.is_null() {
                return -3;
            }
            let outlet = unsafe { &*out };
            let nch = outlet.outlet.info().channel_count() as usize;
            if nch == 0 {
                return -3;
            }
            let slice = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
            let n_samples = slice.len() / nch;
            let srate = outlet.outlet.info().nominal_srate();
            let mut ts = if timestamp == 0.0 {
                local_clock()
            } else {
                timestamp
            };
            if srate != IRREGULAR_RATE && n_samples > 1 {
                ts -= (n_samples - 1) as f64 / srate;
            }
            for i in 0..n_samples {
                let chunk = &slice[i * nch..(i + 1) * nch];
                let sample_ts = if i == 0 { ts } else { DEDUCED_TIMESTAMP };
                let is_last = i == n_samples - 1;
                outlet
                    .outlet
                    .$method(chunk, sample_ts, pushthrough != 0 && is_last);
            }
            0
        }
        #[no_mangle]
        pub extern "C" fn $fnametn(
            out: *mut LslOutlet,
            data: *const $ty,
            data_elements: c_ulong,
            timestamps: *const c_double,
        ) -> i32 {
            $fnametnp(out, data, data_elements, timestamps, 1)
        }
        #[no_mangle]
        pub extern "C" fn $fnametnp(
            out: *mut LslOutlet,
            data: *const $ty,
            data_elements: c_ulong,
            timestamps: *const c_double,
            pushthrough: i32,
        ) -> i32 {
            if out.is_null() || data.is_null() || timestamps.is_null() {
                return -3;
            }
            let outlet = unsafe { &*out };
            let nch = outlet.outlet.info().channel_count() as usize;
            if nch == 0 {
                return -3;
            }
            let slice = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
            let n_samples = slice.len() / nch;
            let ts_slice = unsafe { std::slice::from_raw_parts(timestamps, n_samples) };
            for i in 0..n_samples {
                let chunk = &slice[i * nch..(i + 1) * nch];
                let is_last = i == n_samples - 1;
                outlet
                    .outlet
                    .$method(chunk, ts_slice[i], pushthrough != 0 && is_last);
            }
            0
        }
    };
}

impl_push_chunk!(
    lsl_push_chunk_f,
    lsl_push_chunk_ft,
    lsl_push_chunk_ftp,
    lsl_push_chunk_ftn,
    lsl_push_chunk_ftnp,
    c_float,
    push_sample_f
);
impl_push_chunk!(
    lsl_push_chunk_d,
    lsl_push_chunk_dt,
    lsl_push_chunk_dtp,
    lsl_push_chunk_dtn,
    lsl_push_chunk_dtnp,
    c_double,
    push_sample_d
);
impl_push_chunk!(
    lsl_push_chunk_l,
    lsl_push_chunk_lt,
    lsl_push_chunk_ltp,
    lsl_push_chunk_ltn,
    lsl_push_chunk_ltnp,
    i64,
    push_sample_i64
);
impl_push_chunk!(
    lsl_push_chunk_i,
    lsl_push_chunk_it,
    lsl_push_chunk_itp,
    lsl_push_chunk_itn,
    lsl_push_chunk_itnp,
    i32,
    push_sample_i32
);
impl_push_chunk!(
    lsl_push_chunk_s,
    lsl_push_chunk_st,
    lsl_push_chunk_stp,
    lsl_push_chunk_stn,
    lsl_push_chunk_stnp,
    i16,
    push_sample_i16
);

// char chunks — treat each char as one i8 channel value
#[no_mangle]
pub extern "C" fn lsl_push_chunk_c(
    out: *mut LslOutlet,
    data: *const c_char,
    data_elements: c_ulong,
) -> i32 {
    lsl_push_chunk_ctp(out, data, data_elements, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_ct(
    out: *mut LslOutlet,
    data: *const c_char,
    data_elements: c_ulong,
    timestamp: c_double,
) -> i32 {
    lsl_push_chunk_ctp(out, data, data_elements, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_ctp(
    out: *mut LslOutlet,
    data: *const c_char,
    data_elements: c_ulong,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let slice = unsafe { std::slice::from_raw_parts(data as *const i8, data_elements as usize) };
    let n_samples = slice.len() / nch;
    for i in 0..n_samples {
        let chunk: Vec<i16> = slice[i * nch..(i + 1) * nch]
            .iter()
            .map(|&v| v as i16)
            .collect();
        let ts = if i == 0 { timestamp } else { DEDUCED_TIMESTAMP };
        outlet
            .outlet
            .push_sample_i16(&chunk, ts, pushthrough != 0 && i == n_samples - 1);
    }
    0
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_ctn(
    out: *mut LslOutlet,
    data: *const c_char,
    data_elements: c_ulong,
    timestamps: *const c_double,
) -> i32 {
    lsl_push_chunk_ctnp(out, data, data_elements, timestamps, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_ctnp(
    out: *mut LslOutlet,
    data: *const c_char,
    data_elements: c_ulong,
    timestamps: *const c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() || timestamps.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let slice = unsafe { std::slice::from_raw_parts(data as *const i8, data_elements as usize) };
    let n_samples = slice.len() / nch;
    let ts_slice = unsafe { std::slice::from_raw_parts(timestamps, n_samples) };
    for i in 0..n_samples {
        let chunk: Vec<i16> = slice[i * nch..(i + 1) * nch]
            .iter()
            .map(|&v| v as i16)
            .collect();
        outlet
            .outlet
            .push_sample_i16(&chunk, ts_slice[i], pushthrough != 0 && i == n_samples - 1);
    }
    0
}

// string chunks
#[no_mangle]
pub extern "C" fn lsl_push_chunk_str(
    out: *mut LslOutlet,
    data: *const *const c_char,
    data_elements: c_ulong,
) -> i32 {
    lsl_push_chunk_strtp(out, data, data_elements, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_strt(
    out: *mut LslOutlet,
    data: *const *const c_char,
    data_elements: c_ulong,
    timestamp: c_double,
) -> i32 {
    lsl_push_chunk_strtp(out, data, data_elements, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_strtp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    data_elements: c_ulong,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let ptrs = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
    let n_samples = ptrs.len() / nch;
    for i in 0..n_samples {
        let strings: Vec<String> = ptrs[i * nch..(i + 1) * nch]
            .iter()
            .map(|&p| cstr_to_string(p))
            .collect();
        let ts = if i == 0 { timestamp } else { DEDUCED_TIMESTAMP };
        outlet
            .outlet
            .push_sample_str(&strings, ts, pushthrough != 0 && i == n_samples - 1);
    }
    0
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_strtn(
    out: *mut LslOutlet,
    data: *const *const c_char,
    data_elements: c_ulong,
    timestamps: *const c_double,
) -> i32 {
    lsl_push_chunk_strtnp(out, data, data_elements, timestamps, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_strtnp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    data_elements: c_ulong,
    timestamps: *const c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() || timestamps.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let ptrs = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
    let n_samples = ptrs.len() / nch;
    let ts_slice = unsafe { std::slice::from_raw_parts(timestamps, n_samples) };
    for i in 0..n_samples {
        let strings: Vec<String> = ptrs[i * nch..(i + 1) * nch]
            .iter()
            .map(|&p| cstr_to_string(p))
            .collect();
        outlet.outlet.push_sample_str(
            &strings,
            ts_slice[i],
            pushthrough != 0 && i == n_samples - 1,
        );
    }
    0
}

// buf chunks
#[no_mangle]
pub extern "C" fn lsl_push_chunk_buf(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    data_elements: c_ulong,
) -> i32 {
    lsl_push_chunk_buftp(out, data, lengths, data_elements, 0.0, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_buft(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    data_elements: c_ulong,
    timestamp: c_double,
) -> i32 {
    lsl_push_chunk_buftp(out, data, lengths, data_elements, timestamp, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_buftp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    data_elements: c_ulong,
    timestamp: c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() || lengths.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let ptrs = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
    let lens = unsafe { std::slice::from_raw_parts(lengths, data_elements as usize) };
    let n_samples = ptrs.len() / nch;
    for i in 0..n_samples {
        let strings: Vec<String> = (0..nch)
            .map(|j| {
                let idx = i * nch + j;
                let bytes = unsafe {
                    std::slice::from_raw_parts(ptrs[idx] as *const u8, lens[idx] as usize)
                };
                String::from_utf8_lossy(bytes).into_owned()
            })
            .collect();
        let ts = if i == 0 { timestamp } else { DEDUCED_TIMESTAMP };
        outlet
            .outlet
            .push_sample_str(&strings, ts, pushthrough != 0 && i == n_samples - 1);
    }
    0
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_buftn(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    data_elements: c_ulong,
    timestamps: *const c_double,
) -> i32 {
    lsl_push_chunk_buftnp(out, data, lengths, data_elements, timestamps, 1)
}
#[no_mangle]
pub extern "C" fn lsl_push_chunk_buftnp(
    out: *mut LslOutlet,
    data: *const *const c_char,
    lengths: *const u32,
    data_elements: c_ulong,
    timestamps: *const c_double,
    pushthrough: i32,
) -> i32 {
    if out.is_null() || data.is_null() || lengths.is_null() || timestamps.is_null() {
        return -3;
    }
    let outlet = unsafe { &*out };
    let nch = outlet.outlet.info().channel_count() as usize;
    if nch == 0 {
        return -3;
    }
    let ptrs = unsafe { std::slice::from_raw_parts(data, data_elements as usize) };
    let lens = unsafe { std::slice::from_raw_parts(lengths, data_elements as usize) };
    let n_samples = ptrs.len() / nch;
    let ts_slice = unsafe { std::slice::from_raw_parts(timestamps, n_samples) };
    for i in 0..n_samples {
        let strings: Vec<String> = (0..nch)
            .map(|j| {
                let idx = i * nch + j;
                let bytes = unsafe {
                    std::slice::from_raw_parts(ptrs[idx] as *const u8, lens[idx] as usize)
                };
                String::from_utf8_lossy(bytes).into_owned()
            })
            .collect();
        outlet.outlet.push_sample_str(
            &strings,
            ts_slice[i],
            pushthrough != 0 && i == n_samples - 1,
        );
    }
    0
}

#[no_mangle]
pub extern "C" fn lsl_have_consumers(out: *mut LslOutlet) -> i32 {
    if out.is_null() {
        return 0;
    }
    unsafe { &*out }.outlet.have_consumers() as i32
}

#[no_mangle]
pub extern "C" fn lsl_wait_for_consumers(out: *mut LslOutlet, timeout: c_double) -> i32 {
    if out.is_null() {
        return 0;
    }
    unsafe { &*out }.outlet.wait_for_consumers(timeout) as i32
}

// === inlet.h ===

#[no_mangle]
pub extern "C" fn lsl_create_inlet(
    info: *mut LslStreamInfo,
    max_buflen: i32,
    max_chunklen: i32,
    recover: i32,
) -> *mut LslInlet {
    null_check!(info);
    let info = unsafe { &*info };
    let inlet = StreamInlet::new(&info.info, max_buflen, max_chunklen, recover != 0);
    Box::into_raw(Box::new(LslInlet { inlet }))
}

#[no_mangle]
pub extern "C" fn lsl_create_inlet_ex(
    info: *mut LslStreamInfo,
    max_buflen: i32,
    max_chunklen: i32,
    recover: i32,
    _flags: i32,
) -> *mut LslInlet {
    lsl_create_inlet(info, max_buflen, max_chunklen, recover)
}

#[no_mangle]
pub extern "C" fn lsl_destroy_inlet(inlet: *mut LslInlet) {
    if !inlet.is_null() {
        unsafe {
            let _ = Box::from_raw(inlet);
        }
    }
}

#[no_mangle]
pub extern "C" fn lsl_get_fullinfo(
    inlet: *mut LslInlet,
    timeout: c_double,
    ec: *mut i32,
) -> *mut LslStreamInfo {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    null_check!(inlet);
    let info = unsafe { &*inlet }.inlet.get_fullinfo(timeout);
    Box::into_raw(Box::new(LslStreamInfo { info }))
}

#[no_mangle]
pub extern "C" fn lsl_open_stream(inlet: *mut LslInlet, timeout: c_double, ec: *mut i32) {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() {
        return;
    }
    let result = unsafe { &*inlet }.inlet.open_stream(timeout);
    if let Err(_) = result {
        if !ec.is_null() {
            unsafe {
                *ec = ErrorCode::TimeoutError as i32;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn lsl_close_stream(inlet: *mut LslInlet) {
    if inlet.is_null() {
        return;
    }
    unsafe { &*inlet }.inlet.close_stream();
}

#[no_mangle]
pub extern "C" fn lsl_time_correction(
    inlet: *mut LslInlet,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() {
        return 0.0;
    }
    unsafe { &*inlet }.inlet.time_correction(timeout)
}

#[no_mangle]
pub extern "C" fn lsl_time_correction_ex(
    inlet: *mut LslInlet,
    remote_time: *mut c_double,
    uncertainty: *mut c_double,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if !remote_time.is_null() {
        unsafe {
            *remote_time = 0.0;
        }
    }
    if !uncertainty.is_null() {
        unsafe {
            *uncertainty = 0.0;
        }
    }
    if inlet.is_null() {
        return 0.0;
    }
    unsafe { &*inlet }.inlet.time_correction(timeout)
}

#[no_mangle]
pub extern "C" fn lsl_set_postprocessing(inlet: *mut LslInlet, flags: u32) -> i32 {
    if inlet.is_null() {
        return -3;
    }
    unsafe { &*inlet }.inlet.set_postprocessing(flags);
    0
}

macro_rules! impl_pull_sample {
    ($fname:ident, $ty:ty, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fname(
            inlet: *mut LslInlet,
            buffer: *mut $ty,
            buffer_elements: i32,
            timeout: c_double,
            ec: *mut i32,
        ) -> c_double {
            if !ec.is_null() {
                unsafe {
                    *ec = 0;
                }
            }
            if inlet.is_null() || buffer.is_null() {
                return 0.0;
            }
            let inlet = unsafe { &*inlet };
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_elements as usize) };
            match inlet.inlet.$method(buf, timeout) {
                Ok(ts) => ts,
                Err(_) => {
                    if !ec.is_null() {
                        unsafe {
                            *ec = ErrorCode::InternalError as i32;
                        }
                    }
                    0.0
                }
            }
        }
    };
}

impl_pull_sample!(lsl_pull_sample_f, c_float, pull_sample_f);
impl_pull_sample!(lsl_pull_sample_d, c_double, pull_sample_d);
impl_pull_sample!(lsl_pull_sample_i, i32, pull_sample_i32);
impl_pull_sample!(lsl_pull_sample_s, i16, pull_sample_i16);
impl_pull_sample!(lsl_pull_sample_l, i64, pull_sample_i64);

#[no_mangle]
pub extern "C" fn lsl_pull_sample_c(
    inlet: *mut LslInlet,
    buffer: *mut c_char,
    buffer_elements: i32,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    // Pull as i8
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || buffer.is_null() {
        return 0.0;
    }
    let mut tmp = vec![0i16; buffer_elements as usize];
    let inlet_ref = unsafe { &*inlet };
    match inlet_ref.inlet.pull_sample_i16(&mut tmp, timeout) {
        Ok(ts) => {
            let dst = unsafe {
                std::slice::from_raw_parts_mut(buffer as *mut u8, buffer_elements as usize)
            };
            for (d, s) in dst.iter_mut().zip(tmp.iter()) {
                *d = *s as u8;
            }
            ts
        }
        Err(_) => 0.0,
    }
}

#[no_mangle]
pub extern "C" fn lsl_pull_sample_str(
    inlet: *mut LslInlet,
    buffer: *mut *mut c_char,
    buffer_elements: i32,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || buffer.is_null() {
        return 0.0;
    }
    let inlet_ref = unsafe { &*inlet };
    match inlet_ref.inlet.pull_sample_str(timeout) {
        Ok((strings, ts)) => {
            let n = strings.len().min(buffer_elements as usize);
            for i in 0..n {
                let cstr =
                    CString::new(strings[i].as_str()).unwrap_or_else(|_| CString::new("").unwrap());
                unsafe {
                    *buffer.add(i) = cstr.into_raw();
                }
            }
            ts
        }
        Err(_) => 0.0,
    }
}

#[no_mangle]
pub extern "C" fn lsl_pull_sample_buf(
    inlet: *mut LslInlet,
    buffer: *mut *mut c_char,
    buffer_lengths: *mut u32,
    buffer_elements: i32,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || buffer.is_null() {
        return 0.0;
    }
    let inlet_ref = unsafe { &*inlet };
    match inlet_ref.inlet.pull_sample_str(timeout) {
        Ok((strings, ts)) => {
            let n = strings.len().min(buffer_elements as usize);
            for i in 0..n {
                let bytes = strings[i].as_bytes();
                let layout = std::alloc::Layout::from_size_align(bytes.len().max(1), 1).unwrap();
                let ptr = unsafe { std::alloc::alloc(layout) };
                if !ptr.is_null() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                        *buffer.add(i) = ptr as *mut c_char;
                        if !buffer_lengths.is_null() {
                            *buffer_lengths.add(i) = bytes.len() as u32;
                        }
                    }
                }
            }
            ts
        }
        Err(_) => 0.0,
    }
}

#[no_mangle]
pub extern "C" fn lsl_pull_sample_v(
    inlet: *mut LslInlet,
    buffer: *mut c_void,
    buffer_bytes: i32,
    timeout: c_double,
    ec: *mut i32,
) -> c_double {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || buffer.is_null() {
        return 0.0;
    }
    let inlet_ref = unsafe { &*inlet };
    let info = inlet_ref.inlet.get_fullinfo(0.0);
    let fmt = info.channel_format();
    let nch = info.channel_count() as usize;
    match fmt {
        ChannelFormat::Float32 => {
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer as *mut f32, nch) };
            inlet_ref.inlet.pull_sample_f(buf, timeout).unwrap_or(0.0)
        }
        ChannelFormat::Double64 => {
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer as *mut f64, nch) };
            inlet_ref.inlet.pull_sample_d(buf, timeout).unwrap_or(0.0)
        }
        ChannelFormat::Int32 => {
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer as *mut i32, nch) };
            inlet_ref.inlet.pull_sample_i32(buf, timeout).unwrap_or(0.0)
        }
        ChannelFormat::Int16 => {
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer as *mut i16, nch) };
            inlet_ref.inlet.pull_sample_i16(buf, timeout).unwrap_or(0.0)
        }
        ChannelFormat::Int64 => {
            let buf = unsafe { std::slice::from_raw_parts_mut(buffer as *mut i64, nch) };
            inlet_ref.inlet.pull_sample_i64(buf, timeout).unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

// === Pull chunk functions ===

macro_rules! impl_pull_chunk {
    ($fname:ident, $ty:ty, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fname(
            inlet: *mut LslInlet,
            data_buffer: *mut $ty,
            timestamp_buffer: *mut c_double,
            data_buffer_elements: c_ulong,
            timestamp_buffer_elements: c_ulong,
            timeout: c_double,
            ec: *mut i32,
        ) -> c_ulong {
            if !ec.is_null() {
                unsafe {
                    *ec = 0;
                }
            }
            if inlet.is_null() || data_buffer.is_null() {
                return 0;
            }
            let inlet_ref = unsafe { &*inlet };
            let nch = inlet_ref.inlet.get_fullinfo(0.0).channel_count() as usize;
            if nch == 0 {
                return 0;
            }
            let max_samples = data_buffer_elements as usize / nch;
            let end_time = if timeout > 0.0 {
                local_clock() + timeout
            } else {
                0.0
            };
            let mut samples_written = 0usize;

            for i in 0..max_samples {
                let remaining = if timeout > 0.0 {
                    (end_time - local_clock()).max(0.0)
                } else {
                    0.0
                };
                let buf = unsafe { std::slice::from_raw_parts_mut(data_buffer.add(i * nch), nch) };
                match inlet_ref.inlet.$method(buf, remaining) {
                    Ok(ts) if ts > 0.0 => {
                        if !timestamp_buffer.is_null() {
                            unsafe {
                                *timestamp_buffer.add(i) = ts;
                            }
                        }
                        samples_written += 1;
                    }
                    _ => break,
                }
            }
            (samples_written * nch) as c_ulong
        }
    };
}

impl_pull_chunk!(lsl_pull_chunk_f, c_float, pull_sample_f);
impl_pull_chunk!(lsl_pull_chunk_d, c_double, pull_sample_d);
impl_pull_chunk!(lsl_pull_chunk_l, i64, pull_sample_i64);
impl_pull_chunk!(lsl_pull_chunk_i, i32, pull_sample_i32);
impl_pull_chunk!(lsl_pull_chunk_s, i16, pull_sample_i16);
// char chunk pull
#[no_mangle]
pub extern "C" fn lsl_pull_chunk_c(
    inlet: *mut LslInlet,
    data: *mut c_char,
    ts: *mut c_double,
    data_buffer_elements: c_ulong,
    timestamp_buffer_elements: c_ulong,
    timeout: c_double,
    ec: *mut i32,
) -> c_ulong {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || data.is_null() {
        return 0;
    }
    let inlet_ref = unsafe { &*inlet };
    let nch = inlet_ref.inlet.get_fullinfo(0.0).channel_count() as usize;
    if nch == 0 {
        return 0;
    }
    let max_samples = data_buffer_elements as usize / nch;
    let mut buf = vec![0i16; nch];
    let mut samples_written = 0usize;
    for i in 0..max_samples {
        let remaining = if i == 0 { timeout } else { 0.0 };
        match inlet_ref.inlet.pull_sample_i16(&mut buf, remaining) {
            Ok(t) if t > 0.0 => {
                for j in 0..nch {
                    unsafe {
                        *data.add(i * nch + j) = buf[j] as c_char;
                    }
                }
                if !ts.is_null() {
                    unsafe {
                        *ts.add(i) = t;
                    }
                }
                samples_written += 1;
            }
            _ => break,
        }
    }
    (samples_written * nch) as c_ulong
}

// String chunk pull
#[no_mangle]
pub extern "C" fn lsl_pull_chunk_str(
    inlet: *mut LslInlet,
    data: *mut *mut c_char,
    ts: *mut c_double,
    data_buffer_elements: c_ulong,
    timestamp_buffer_elements: c_ulong,
    timeout: c_double,
    ec: *mut i32,
) -> c_ulong {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || data.is_null() {
        return 0;
    }
    let inlet_ref = unsafe { &*inlet };
    let nch = inlet_ref.inlet.get_fullinfo(0.0).channel_count() as usize;
    if nch == 0 {
        return 0;
    }
    let max_samples = data_buffer_elements as usize / nch;
    let mut samples_written = 0usize;
    for i in 0..max_samples {
        let remaining = if i == 0 { timeout } else { 0.0 };
        match inlet_ref.inlet.pull_sample_str(remaining) {
            Ok((strings, t)) if t > 0.0 => {
                for (j, s) in strings.iter().enumerate().take(nch) {
                    unsafe {
                        *data.add(i * nch + j) = string_to_cstr(s);
                    }
                }
                if !ts.is_null() {
                    unsafe {
                        *ts.add(i) = t;
                    }
                }
                samples_written += 1;
            }
            _ => break,
        }
    }
    (samples_written * nch) as c_ulong
}

// Buf chunk pull
#[no_mangle]
pub extern "C" fn lsl_pull_chunk_buf(
    inlet: *mut LslInlet,
    data: *mut *mut c_char,
    lengths: *mut u32,
    ts: *mut c_double,
    data_buffer_elements: c_ulong,
    timestamp_buffer_elements: c_ulong,
    timeout: c_double,
    ec: *mut i32,
) -> c_ulong {
    if !ec.is_null() {
        unsafe {
            *ec = 0;
        }
    }
    if inlet.is_null() || data.is_null() {
        return 0;
    }
    let inlet_ref = unsafe { &*inlet };
    let nch = inlet_ref.inlet.get_fullinfo(0.0).channel_count() as usize;
    if nch == 0 {
        return 0;
    }
    let max_samples = data_buffer_elements as usize / nch;
    let mut samples_written = 0usize;
    for i in 0..max_samples {
        let remaining = if i == 0 { timeout } else { 0.0 };
        match inlet_ref.inlet.pull_sample_str(remaining) {
            Ok((strings, t)) if t > 0.0 => {
                for (j, s) in strings.iter().enumerate().take(nch) {
                    let bytes = s.as_bytes();
                    let layout =
                        std::alloc::Layout::from_size_align(bytes.len().max(1), 1).unwrap();
                    let ptr = unsafe { std::alloc::alloc(layout) };
                    if !ptr.is_null() {
                        unsafe {
                            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                            *data.add(i * nch + j) = ptr as *mut c_char;
                            if !lengths.is_null() {
                                *lengths.add(i * nch + j) = bytes.len() as u32;
                            }
                        }
                    }
                }
                if !ts.is_null() {
                    unsafe {
                        *ts.add(i) = t;
                    }
                }
                samples_written += 1;
            }
            _ => break,
        }
    }
    (samples_written * nch) as c_ulong
}

#[no_mangle]
pub extern "C" fn lsl_samples_available(inlet: *mut LslInlet) -> u32 {
    if inlet.is_null() {
        return 0;
    }
    unsafe { &*inlet }.inlet.samples_available()
}

#[no_mangle]
pub extern "C" fn lsl_inlet_flush(inlet: *mut LslInlet) -> u32 {
    if inlet.is_null() {
        return 0;
    }
    unsafe { &*inlet }.inlet.flush()
}

#[no_mangle]
pub extern "C" fn lsl_was_clock_reset(inlet: *mut LslInlet) -> u32 {
    if inlet.is_null() {
        return 0;
    }
    unsafe { &*inlet }.inlet.was_clock_reset() as u32
}

#[no_mangle]
pub extern "C" fn lsl_smoothing_halftime(inlet: *mut LslInlet, value: c_float) -> i32 {
    if inlet.is_null() {
        return -3;
    }
    unsafe { &*inlet }.inlet.smoothing_halftime(value);
    0
}

// === resolver.h ===

#[no_mangle]
pub extern "C" fn lsl_resolve_all(
    buffer: *mut *mut LslStreamInfo,
    buffer_elements: u32,
    wait_time: c_double,
) -> i32 {
    if buffer.is_null() {
        return 0;
    }
    let results = resolver::resolve_all(wait_time);
    let n = results.len().min(buffer_elements as usize);
    for (i, info) in results.into_iter().take(n).enumerate() {
        unsafe {
            *buffer.add(i) = Box::into_raw(Box::new(LslStreamInfo { info }));
        }
    }
    n as i32
}

#[no_mangle]
pub extern "C" fn lsl_resolve_byprop(
    buffer: *mut *mut LslStreamInfo,
    buffer_elements: u32,
    prop: *const c_char,
    value: *const c_char,
    minimum: i32,
    timeout: c_double,
) -> i32 {
    if buffer.is_null() {
        return 0;
    }
    let prop = cstr_to_string(prop);
    let value = cstr_to_string(value);
    let results = resolver::resolve_by_property(&prop, &value, minimum, timeout);
    let n = results.len().min(buffer_elements as usize);
    for (i, info) in results.into_iter().take(n).enumerate() {
        unsafe {
            *buffer.add(i) = Box::into_raw(Box::new(LslStreamInfo { info }));
        }
    }
    n as i32
}

#[no_mangle]
pub extern "C" fn lsl_resolve_bypred(
    buffer: *mut *mut LslStreamInfo,
    buffer_elements: u32,
    pred: *const c_char,
    minimum: i32,
    timeout: c_double,
) -> i32 {
    if buffer.is_null() {
        return 0;
    }
    let pred = cstr_to_string(pred);
    let results = resolver::resolve_by_predicate(&pred, minimum, timeout);
    let n = results.len().min(buffer_elements as usize);
    for (i, info) in results.into_iter().take(n).enumerate() {
        unsafe {
            *buffer.add(i) = Box::into_raw(Box::new(LslStreamInfo { info }));
        }
    }
    n as i32
}

#[no_mangle]
pub extern "C" fn lsl_create_continuous_resolver(
    forget_after: c_double,
) -> *mut LslContinuousResolver {
    let resolver = ContinuousResolver::new("", forget_after);
    Box::into_raw(Box::new(LslContinuousResolver { resolver }))
}

#[no_mangle]
pub extern "C" fn lsl_create_continuous_resolver_byprop(
    prop: *const c_char,
    value: *const c_char,
    forget_after: c_double,
) -> *mut LslContinuousResolver {
    let prop = cstr_to_string(prop);
    let value = cstr_to_string(value);
    let query = if value.is_empty() {
        String::new()
    } else {
        format!("{}='{}'", prop, value)
    };
    let resolver = ContinuousResolver::new(&query, forget_after);
    Box::into_raw(Box::new(LslContinuousResolver { resolver }))
}

#[no_mangle]
pub extern "C" fn lsl_create_continuous_resolver_bypred(
    pred: *const c_char,
    forget_after: c_double,
) -> *mut LslContinuousResolver {
    let pred = cstr_to_string(pred);
    let resolver = ContinuousResolver::new(&pred, forget_after);
    Box::into_raw(Box::new(LslContinuousResolver { resolver }))
}

#[no_mangle]
pub extern "C" fn lsl_resolver_results(
    res: *mut LslContinuousResolver,
    buffer: *mut *mut LslStreamInfo,
    buffer_elements: u32,
) -> i32 {
    if res.is_null() || buffer.is_null() {
        return 0;
    }
    let results = unsafe { &*res }.resolver.results();
    let n = results.len().min(buffer_elements as usize);
    for (i, info) in results.into_iter().take(n).enumerate() {
        unsafe {
            *buffer.add(i) = Box::into_raw(Box::new(LslStreamInfo { info }));
        }
    }
    n as i32
}

#[no_mangle]
pub extern "C" fn lsl_destroy_continuous_resolver(res: *mut LslContinuousResolver) {
    if !res.is_null() {
        unsafe {
            let _ = Box::from_raw(res);
        }
    }
}

// === xml.h ===

#[no_mangle]
pub extern "C" fn lsl_first_child(e: *mut LslXmlPtr) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.first_child();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_last_child(e: *mut LslXmlPtr) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.last_child();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_next_sibling(e: *mut LslXmlPtr) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.next_sibling();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_previous_sibling(e: *mut LslXmlPtr) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.previous_sibling();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_parent(e: *mut LslXmlPtr) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.parent();
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_child(e: *mut LslXmlPtr, name: *const c_char) -> *mut LslXmlPtr {
    null_check!(e);
    let name = cstr_to_string(name);
    let node = unsafe { &*e }.node.child(&name);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_next_sibling_n(e: *mut LslXmlPtr, name: *const c_char) -> *mut LslXmlPtr {
    null_check!(e);
    let name = cstr_to_string(name);
    let node = unsafe { &*e }.node.next_sibling_named(&name);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_previous_sibling_n(e: *mut LslXmlPtr, name: *const c_char) -> *mut LslXmlPtr {
    null_check!(e);
    let name = cstr_to_string(name);
    let node = unsafe { &*e }.node.previous_sibling_named(&name);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_empty(e: *mut LslXmlPtr) -> i32 {
    if e.is_null() {
        return 1;
    }
    unsafe { &*e }.node.is_empty() as i32
}

#[no_mangle]
pub extern "C" fn lsl_is_text(e: *mut LslXmlPtr) -> i32 {
    if e.is_null() {
        return 0;
    }
    // A text node has no name but has a value
    let node = unsafe { &*e };
    (node.node.name().is_empty() && !node.node.value().is_empty()) as i32
}

#[no_mangle]
pub extern "C" fn lsl_name(e: *mut LslXmlPtr) -> *const c_char {
    if e.is_null() {
        return ptr::null();
    }
    string_to_cstr(&unsafe { &*e }.node.name())
}

#[no_mangle]
pub extern "C" fn lsl_value(e: *mut LslXmlPtr) -> *const c_char {
    if e.is_null() {
        return ptr::null();
    }
    string_to_cstr(&unsafe { &*e }.node.value())
}

#[no_mangle]
pub extern "C" fn lsl_child_value(e: *mut LslXmlPtr) -> *const c_char {
    if e.is_null() {
        return ptr::null();
    }
    string_to_cstr(&unsafe { &*e }.node.value())
}

#[no_mangle]
pub extern "C" fn lsl_child_value_n(e: *mut LslXmlPtr, name: *const c_char) -> *const c_char {
    if e.is_null() {
        return ptr::null();
    }
    let name = cstr_to_string(name);
    string_to_cstr(&unsafe { &*e }.node.child_value(&name))
}

#[no_mangle]
pub extern "C" fn lsl_append_child_value(
    e: *mut LslXmlPtr,
    name: *const c_char,
    value: *const c_char,
) -> *mut LslXmlPtr {
    null_check!(e);
    let name = cstr_to_string(name);
    let value = cstr_to_string(value);
    let node = unsafe { &*e }.node.append_child_value(&name, &value);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_prepend_child_value(
    e: *mut LslXmlPtr,
    name: *const c_char,
    value: *const c_char,
) -> *mut LslXmlPtr {
    null_check!(e);
    let name = cstr_to_string(name);
    let value = cstr_to_string(value);
    let node = unsafe { &*e }.node.prepend_child_value(&name, &value);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_set_child_value(
    e: *mut LslXmlPtr,
    name: *const c_char,
    value: *const c_char,
) -> i32 {
    if e.is_null() {
        return 0;
    }
    let name = cstr_to_string(name);
    let value = cstr_to_string(value);
    unsafe { &*e }.node.set_child_value(&name, &value) as i32
}

#[no_mangle]
pub extern "C" fn lsl_set_name(e: *mut LslXmlPtr, rhs: *const c_char) -> i32 {
    if e.is_null() {
        return 0;
    }
    unsafe { &*e }.node.set_name(&cstr_to_string(rhs));
    1
}

#[no_mangle]
pub extern "C" fn lsl_set_value(e: *mut LslXmlPtr, rhs: *const c_char) -> i32 {
    if e.is_null() {
        return 0;
    }
    unsafe { &*e }.node.set_value(&cstr_to_string(rhs));
    1
}

#[no_mangle]
pub extern "C" fn lsl_append_child(e: *mut LslXmlPtr, name: *const c_char) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.append_child(&cstr_to_string(name));
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_prepend_child(e: *mut LslXmlPtr, name: *const c_char) -> *mut LslXmlPtr {
    null_check!(e);
    let node = unsafe { &*e }.node.prepend_child(&cstr_to_string(name));
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_append_copy(e: *mut LslXmlPtr, e2: *mut LslXmlPtr) -> *mut LslXmlPtr {
    if e.is_null() || e2.is_null() {
        return ptr::null_mut();
    }
    let node = unsafe { &*e }.node.append_copy(&unsafe { &*e2 }.node);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_prepend_copy(e: *mut LslXmlPtr, e2: *mut LslXmlPtr) -> *mut LslXmlPtr {
    if e.is_null() || e2.is_null() {
        return ptr::null_mut();
    }
    let node = unsafe { &*e }.node.prepend_copy(&unsafe { &*e2 }.node);
    Box::into_raw(Box::new(LslXmlPtr { node }))
}

#[no_mangle]
pub extern "C" fn lsl_remove_child_n(e: *mut LslXmlPtr, name: *const c_char) {
    if e.is_null() {
        return;
    }
    unsafe { &*e }
        .node
        .remove_child_named(&cstr_to_string(name));
}

#[no_mangle]
pub extern "C" fn lsl_remove_child(e: *mut LslXmlPtr, e2: *mut LslXmlPtr) {
    if e.is_null() || e2.is_null() {
        return;
    }
    unsafe { &*e }.node.remove_child(&unsafe { &*e2 }.node);
}
