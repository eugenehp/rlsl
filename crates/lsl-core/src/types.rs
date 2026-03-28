//! Core type definitions matching liblsl's types.

/// The protocol version (1.10 = 110)
pub const LSL_PROTOCOL_VERSION: i32 = 110;
/// The library version
pub const LSL_LIBRARY_VERSION: i32 = 117;
/// Constant for streams with irregular sampling rate
pub const IRREGULAR_RATE: f64 = 0.0;
/// Constant to indicate that the timestamp should be deduced
pub const DEDUCED_TIMESTAMP: f64 = -1.0;
/// A very large timeout
pub const FOREVER: f64 = 32000000.0;

/// Default multicast port
pub const MULTICAST_PORT: u16 = 16571;
/// Default base port for TCP/UDP services
pub const BASE_PORT: u16 = 16572;
/// Default port range
pub const PORT_RANGE: u16 = 32;

/// Channel data format of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ChannelFormat {
    Undefined = 0,
    Float32 = 1,
    Double64 = 2,
    String = 3,
    Int32 = 4,
    Int16 = 5,
    Int8 = 6,
    Int64 = 7,
}

impl ChannelFormat {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Float32,
            2 => Self::Double64,
            3 => Self::String,
            4 => Self::Int32,
            5 => Self::Int16,
            6 => Self::Int8,
            7 => Self::Int64,
            _ => Self::Undefined,
        }
    }

    pub fn from_name(s: &str) -> Self {
        match s {
            "float32" => Self::Float32,
            "double64" => Self::Double64,
            "string" => Self::String,
            "int32" => Self::Int32,
            "int16" => Self::Int16,
            "int8" => Self::Int8,
            "int64" => Self::Int64,
            _ => Self::Undefined,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Double64 => "double64",
            Self::String => "string",
            Self::Int32 => "int32",
            Self::Int16 => "int16",
            Self::Int8 => "int8",
            Self::Int64 => "int64",
            Self::Undefined => "undefined",
        }
    }

    /// Bytes per channel value (0 for string)
    pub fn channel_bytes(&self) -> usize {
        match self {
            Self::Float32 => 4,
            Self::Double64 => 8,
            Self::String => 0,
            Self::Int32 => 4,
            Self::Int16 => 2,
            Self::Int8 => 1,
            Self::Int64 => 8,
            Self::Undefined => 0,
        }
    }
}

/// Error codes compatible with liblsl
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    NoError = 0,
    TimeoutError = -1,
    LostError = -2,
    ArgumentError = -3,
    InternalError = -4,
}

/// Post-processing flags for timestamps
pub const PROC_NONE: u32 = 0;
pub const PROC_CLOCKSYNC: u32 = 1;
pub const PROC_DEJITTER: u32 = 2;
pub const PROC_MONOTONIZE: u32 = 4;
pub const PROC_THREADSAFE: u32 = 8;
pub const PROC_ALL: u32 = 1 | 2 | 4 | 8;

/// Transport option flags
pub const TRANSP_DEFAULT: i32 = 0;
pub const TRANSP_BUFSIZE_SAMPLES: i32 = 1;
pub const TRANSP_BUFSIZE_THOUSANDTHS: i32 = 2;

/// Tags for sample serialization protocol
pub const TAG_DEDUCED_TIMESTAMP: u8 = 1;
pub const TAG_TRANSMITTED_TIMESTAMP: u8 = 2;
