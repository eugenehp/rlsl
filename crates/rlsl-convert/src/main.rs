//! `lsl-convert` — Offline format converter for LSL recordings.
//!
//! Usage:
//!   lsl-convert input.xdf --to parquet -o output_dir/
//!   lsl-convert input_dir/ --to csv -o output.csv     (Parquet dir → CSV)
//!   lsl-convert input.xdf --to csv -o output.csv
//!   lsl-convert input.xdf --info                       (print stream info)

use anyhow::{Context, Result};
use arrow::array::*;
use arrow::csv::WriterBuilder as CsvWriterBuilder;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::io::Write;
use std::sync::Arc;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: lsl-convert <input> [--to parquet|csv|info] [-o output]");
        std::process::exit(1);
    }

    let input = &args[1];
    let format = args
        .iter()
        .position(|a| a == "--to")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("info");
    let output = args
        .iter()
        .position(|a| a == "-o")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    if format == "info" || args.contains(&"--info".to_string()) {
        return xdf_info(input);
    }

    match (input_type(input), format) {
        (InputType::Xdf, "parquet") => xdf_to_parquet(input, output.unwrap_or("output_parquet"))?,
        (InputType::Xdf, "csv") => xdf_to_csv(input, output.unwrap_or("output.csv"))?,
        (InputType::Parquet, "csv") => parquet_to_csv(input, output.unwrap_or("output.csv"))?,
        _ => anyhow::bail!("Unsupported conversion: {} → {}", input, format),
    }

    Ok(())
}

#[derive(Debug)]
enum InputType {
    Xdf,
    Parquet,
    Unknown,
}

fn input_type(path: &str) -> InputType {
    if path.ends_with(".xdf") {
        InputType::Xdf
    } else if path.ends_with(".parquet") || std::path::Path::new(path).is_dir() {
        InputType::Parquet
    } else {
        InputType::Unknown
    }
}

// ── XDF parsing (minimal) ────────────────────────────────────────────

/// The values of a stream, kept in a type that holds every value of its channel format exactly:
/// floats as `f64`, integers (up to 64 bits) as `i64`, strings as they are.
#[derive(Debug, Clone, PartialEq)]
enum XdfData {
    Float(Vec<f64>),
    Int(Vec<i64>),
    Text(Vec<String>),
}

impl XdfData {
    fn for_format(format: &str) -> Self {
        match format {
            "int8" | "int16" | "int32" | "int64" => Self::Int(Vec::new()),
            "string" => Self::Text(Vec::new()),
            _ => Self::Float(Vec::new()),
        }
    }

    /// The value at flat index `i` as CSV text.
    fn text(&self, i: usize) -> String {
        match self {
            Self::Float(v) => v[i].to_string(),
            Self::Int(v) => v[i].to_string(),
            // quoted when the field would otherwise break the row
            Self::Text(v) if v[i].contains([',', '"', '\n', '\r']) => {
                format!("\"{}\"", v[i].replace('"', "\"\""))
            }
            Self::Text(v) => v[i].clone(),
        }
    }
}

struct XdfStream {
    stream_id: u32,
    name: String,
    nch: usize,
    srate: f64,
    /// `channel_format` of the stream header: float32, double64, int8/16/32/64, string.
    format: String,
    timestamps: Vec<f64>,
    data: XdfData, // flat: [s0ch0, s0ch1, ..., s1ch0, ...]
}

fn parse_xdf(path: &str) -> Result<Vec<XdfStream>> {
    let data = std::fs::read(path).context("Cannot read XDF file")?;
    parse_xdf_bytes(&data)
}

fn parse_xdf_bytes(data: &[u8]) -> Result<Vec<XdfStream>> {
    anyhow::ensure!(
        data.len() >= 4 && &data[..4] == b"XDF:",
        "Not a valid XDF file"
    );

    let mut pos = 4;
    let mut streams: std::collections::HashMap<u32, XdfStream> = std::collections::HashMap::new();

    while pos < data.len() {
        let nlb = *data.get(pos).unwrap_or(&0) as usize;
        pos += 1;
        if pos + nlb > data.len() {
            break;
        }
        let length = match nlb {
            1 => data[pos] as u64,
            4 => u32::from_le_bytes(data[pos..pos + 4].try_into()?) as u64,
            8 => u64::from_le_bytes(data[pos..pos + 8].try_into()?),
            _ => break,
        };
        pos += nlb;
        let chunk_start = pos;
        if pos + 2 > data.len() {
            break;
        }
        let tag = u16::from_le_bytes(data[pos..pos + 2].try_into()?);
        pos = chunk_start + length as usize;
        if pos > data.len() {
            break;
        }

        match tag {
            2 => {
                // StreamHeader
                let sid = u32::from_le_bytes(data[chunk_start + 2..chunk_start + 6].try_into()?);
                let xml =
                    std::str::from_utf8(&data[chunk_start + 6..chunk_start + length as usize])
                        .unwrap_or("");
                let name = extract_xml_tag(xml, "name").unwrap_or_default();
                let nch: usize = extract_xml_tag(xml, "channel_count")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                let srate: f64 = extract_xml_tag(xml, "nominal_srate")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let format = extract_xml_tag(xml, "channel_format")
                    .map(|f| f.trim().to_ascii_lowercase())
                    .unwrap_or_else(|| "float32".into());
                streams.insert(
                    sid,
                    XdfStream {
                        stream_id: sid,
                        name,
                        nch,
                        srate,
                        data: XdfData::for_format(&format),
                        format,
                        timestamps: vec![],
                    },
                );
            }
            3 => {
                // Samples
                let sid = u32::from_le_bytes(data[chunk_start + 2..chunk_start + 6].try_into()?);
                if let Some(stream) = streams.get_mut(&sid) {
                    let content = &data[chunk_start + 6..chunk_start + length as usize];
                    parse_samples(content, stream);
                }
            }
            _ => {}
        }
    }

    Ok(streams.into_values().collect())
}

/// A variable-length integer of the XDF format: one byte giving the number of bytes (1, 4 or
/// 8) and then that many, little-endian. `None` when the data ends first or the size is none of
/// those.
fn read_varlen(content: &[u8], p: &mut usize) -> Option<usize> {
    let nlb = *content.get(*p)? as usize;
    let bytes = content.get(*p + 1..*p + 1 + nlb)?;
    let v = match nlb {
        1 => bytes[0] as u64,
        4 => u32::from_le_bytes(bytes.try_into().ok()?) as u64,
        8 => u64::from_le_bytes(bytes.try_into().ok()?),
        _ => return None,
    };
    *p += 1 + nlb;
    usize::try_from(v).ok()
}

/// Read the samples of one Samples chunk into `stream`, each value in the width its channel
/// format says: 4 bytes for float32 / int32, 8 for double64 / int64, 2 and 1 for int16 / int8,
/// and a length-prefixed string for string streams. A chunk cut short ends the parse at the last
/// complete sample.
fn parse_samples(content: &[u8], stream: &mut XdfStream) {
    let nch = stream.nch;
    let mut p = 0;
    let Some(n_samples) = read_varlen(content, &mut p) else {
        return;
    };

    let mut last_ts = stream.timestamps.last().copied().unwrap_or(0.0);
    let interval = if stream.srate > 0.0 {
        1.0 / stream.srate
    } else {
        0.0
    };

    for _ in 0..n_samples {
        // Timestamp: a length byte, 8 when the sample carries its own stamp, 0 when it follows
        // the previous one by a sample period.
        let Some(&ts_len) = content.get(p) else {
            return;
        };
        p += 1;
        let ts = if ts_len == 8 {
            let Some(b) = content.get(p..p + 8) else {
                return;
            };
            p += 8;
            f64::from_le_bytes(b.try_into().unwrap_or([0; 8]))
        } else {
            last_ts + interval
        };

        // Channel values: all of them or the sample does not count.
        let fixed = |p: &mut usize, n: usize| -> Option<&[u8]> {
            let b = content.get(*p..*p + n)?;
            *p += n;
            Some(b)
        };
        let complete = match &mut stream.data {
            XdfData::Float(values) => {
                let wide = stream.format == "double64";
                let before = values.len();
                for _ in 0..nch {
                    let v = if wide {
                        fixed(&mut p, 8).map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
                    } else {
                        fixed(&mut p, 4)
                            .map(|b| f32::from_le_bytes(b.try_into().unwrap_or([0; 4])) as f64)
                    };
                    match v {
                        Some(v) => values.push(v),
                        None => break,
                    }
                }
                let ok = values.len() == before + nch;
                if !ok {
                    values.truncate(before);
                }
                ok
            }
            XdfData::Int(values) => {
                let width = match stream.format.as_str() {
                    "int64" => 8,
                    "int16" => 2,
                    "int8" => 1,
                    _ => 4,
                };
                let before = values.len();
                for _ in 0..nch {
                    let Some(b) = fixed(&mut p, width) else { break };
                    values.push(match width {
                        8 => i64::from_le_bytes(b.try_into().unwrap_or([0; 8])),
                        2 => i16::from_le_bytes(b.try_into().unwrap_or([0; 2])) as i64,
                        1 => b[0] as i8 as i64,
                        _ => i32::from_le_bytes(b.try_into().unwrap_or([0; 4])) as i64,
                    });
                }
                let ok = values.len() == before + nch;
                if !ok {
                    values.truncate(before);
                }
                ok
            }
            XdfData::Text(values) => {
                let before = values.len();
                for _ in 0..nch {
                    let Some(len) = read_varlen(content, &mut p) else {
                        break;
                    };
                    let Some(b) = fixed(&mut p, len) else { break };
                    values.push(String::from_utf8_lossy(b).into_owned());
                }
                let ok = values.len() == before + nch;
                if !ok {
                    values.truncate(before);
                }
                ok
            }
        };
        if !complete {
            return;
        }
        last_ts = ts;
        stream.timestamps.push(ts);
    }
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    Some(xml[start..end].to_string())
}

// ── Conversions ──────────────────────────────────────────────────────

fn xdf_info(path: &str) -> Result<()> {
    let streams = parse_xdf(path)?;
    eprintln!("XDF file: {}", path);
    eprintln!("{} stream(s):", streams.len());
    for s in &streams {
        let n = s.timestamps.len();
        let ts_range = if n > 0 {
            format!("[{:.4}..{:.4}]", s.timestamps[0], s.timestamps[n - 1])
        } else {
            "[]".into()
        };
        eprintln!(
            "  stream_id={}: name=\"{}\", {}ch, {}Hz, {}, {} samples, ts {}",
            s.stream_id, s.name, s.nch, s.srate, s.format, n, ts_range
        );
    }
    Ok(())
}

fn xdf_to_parquet(input: &str, output_dir: &str) -> Result<()> {
    let streams = parse_xdf(input)?;
    std::fs::create_dir_all(output_dir)?;

    for s in &streams {
        let n = s.timestamps.len();
        if n == 0 {
            continue;
        }

        // Integer and string streams keep their own type, so nothing is rounded on the way.
        // Float streams are Float64 columns, as they have always been (exact for float32 too).
        let column_type = match s.format.as_str() {
            "int8" => DataType::Int8,
            "int16" => DataType::Int16,
            "int32" => DataType::Int32,
            "int64" => DataType::Int64,
            "string" => DataType::Utf8,
            _ => DataType::Float64,
        };
        let mut fields = vec![Field::new("timestamp", DataType::Float64, false)];
        for ch in 0..s.nch {
            fields.push(Field::new(format!("ch{}", ch), column_type.clone(), false));
        }
        let schema = Arc::new(Schema::new(fields));

        let path = format!(
            "{}/stream_{}_{}.parquet",
            output_dir,
            s.stream_id,
            s.name
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>()
        );
        let file = std::fs::File::create(&path)?;
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(Default::default()))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))?;

        let ts_arr = Float64Array::from(s.timestamps.clone());
        let mut cols: Vec<ArrayRef> = vec![Arc::new(ts_arr)];
        for ch in 0..s.nch {
            let at = |i: usize| i * s.nch + ch;
            let col: ArrayRef = match (&s.data, &column_type) {
                (XdfData::Float(v), _) => {
                    Arc::new(Float64Array::from_iter_values((0..n).map(|i| v[at(i)])))
                }
                (XdfData::Int(v), DataType::Int8) => {
                    Arc::new(Int8Array::from_iter_values((0..n).map(|i| v[at(i)] as i8)))
                }
                (XdfData::Int(v), DataType::Int16) => Arc::new(Int16Array::from_iter_values(
                    (0..n).map(|i| v[at(i)] as i16),
                )),
                (XdfData::Int(v), DataType::Int32) => Arc::new(Int32Array::from_iter_values(
                    (0..n).map(|i| v[at(i)] as i32),
                )),
                (XdfData::Int(v), _) => {
                    Arc::new(Int64Array::from_iter_values((0..n).map(|i| v[at(i)])))
                }
                (XdfData::Text(v), _) => Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| v[at(i)].as_str()),
                )),
            };
            cols.push(col);
        }
        let batch = RecordBatch::try_new(schema, cols)?;
        writer.write(&batch)?;
        writer.close()?;
        eprintln!("  ✓ {} ({} samples)", path, n);
    }
    eprintln!("Done: {} → {}", input, output_dir);
    Ok(())
}

fn xdf_to_csv(input: &str, output: &str) -> Result<()> {
    let streams = parse_xdf(input)?;
    let mut file = std::fs::File::create(output)?;

    for s in &streams {
        let n = s.timestamps.len();
        if n == 0 {
            continue;
        }

        // Header
        write!(file, "stream,timestamp")?;
        for ch in 0..s.nch {
            write!(file, ",ch{}", ch)?;
        }
        writeln!(file)?;

        for i in 0..n {
            write!(file, "{},{:.9}", s.name, s.timestamps[i])?;
            for ch in 0..s.nch {
                write!(file, ",{}", s.data.text(i * s.nch + ch))?;
            }
            writeln!(file)?;
        }
    }
    eprintln!("Done: {} → {} ({} streams)", input, output, streams.len());
    Ok(())
}

fn parquet_to_csv(input: &str, output: &str) -> Result<()> {
    use arrow::array::RecordBatchReader;

    // Find .parquet files
    let paths: Vec<_> = if std::path::Path::new(input).is_dir() {
        std::fs::read_dir(input)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "parquet")
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect()
    } else {
        vec![std::path::PathBuf::from(input)]
    };

    let mut out = std::fs::File::create(output)?;
    let mut first = true;

    for path in &paths {
        let file = std::fs::File::open(path)?;
        let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?
            .build()?;
        let schema = reader.schema();

        if first {
            // CSV header
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            writeln!(out, "{}", names.join(","))?;
            first = false;
        }

        let mut csv_writer = CsvWriterBuilder::new().with_header(false).build(&mut out);
        for batch in reader {
            csv_writer.write(&batch?)?;
        }
    }
    eprintln!("Done: {} → {}", input, output);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: u16, content: &[u8]) -> Vec<u8> {
        let mut out = vec![4u8];
        out.extend_from_slice(&((content.len() + 2) as u32).to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(content);
        out
    }

    fn header(id: u32, name: &str, nch: usize, srate: f64, format: &str) -> Vec<u8> {
        let mut c = id.to_le_bytes().to_vec();
        c.extend_from_slice(
            format!(
                "<?xml version=\"1.0\"?><info><name>{name}</name><channel_count>{nch}</channel_count>\
                 <nominal_srate>{srate}</nominal_srate><channel_format>{format}</channel_format></info>"
            )
            .as_bytes(),
        );
        chunk(2, &c)
    }

    /// A Samples chunk: every sample is `(own timestamp or none, value bytes)`.
    fn samples(id: u32, rows: &[(Option<f64>, Vec<u8>)]) -> Vec<u8> {
        let mut c = id.to_le_bytes().to_vec();
        c.extend_from_slice(&[1, rows.len() as u8]);
        for (ts, values) in rows {
            match ts {
                Some(t) => {
                    c.push(8);
                    c.extend_from_slice(&t.to_le_bytes());
                }
                None => c.push(0),
            }
            c.extend_from_slice(values);
        }
        chunk(3, &c)
    }

    fn le<T, const N: usize>(values: &[T], f: fn(&T) -> [u8; N]) -> Vec<u8> {
        values.iter().flat_map(f).collect()
    }

    /// Streams of every width in one file: each is read in its own width and keeps its exact
    /// values — doubles that no `f32` holds, integers that no `f64` holds.
    #[test]
    fn every_channel_format_is_read_in_its_own_width() {
        let doubles = [
            1_700_000_000.123_456_7,
            std::f64::consts::PI,
            -1.0e-300,
            2.5,
        ];
        let longs = [i64::MAX, i64::MIN, (1 << 53) + 1, -7];
        let mut xdf = b"XDF:".to_vec();
        xdf.extend(header(1, "F32", 2, 100.0, "float32"));
        xdf.extend(header(2, "F64", 2, 100.0, "double64"));
        xdf.extend(header(3, "I64", 2, 0.0, "int64"));
        xdf.extend(header(4, "I16", 3, 0.0, "int16"));
        xdf.extend(header(5, "I8", 1, 0.0, "int8"));
        xdf.extend(header(6, "I32", 1, 0.0, "int32"));
        xdf.extend(header(7, "Events", 1, 0.0, "string"));
        xdf.extend(samples(
            1,
            &[
                (Some(10.0), le(&[1.5f32, -2.25], |v| v.to_le_bytes())),
                (None, le(&[3.0f32, 4.0], |v| v.to_le_bytes())),
            ],
        ));
        xdf.extend(samples(
            2,
            &[
                (Some(10.0), le(&doubles[..2], |v| v.to_le_bytes())),
                (None, le(&doubles[2..], |v| v.to_le_bytes())),
            ],
        ));
        xdf.extend(samples(
            3,
            &[
                (Some(11.0), le(&longs[..2], |v| v.to_le_bytes())),
                (Some(12.0), le(&longs[2..], |v| v.to_le_bytes())),
            ],
        ));
        xdf.extend(samples(
            4,
            &[(
                Some(1.0),
                le(&[i16::MIN, -1, i16::MAX], |v| v.to_le_bytes()),
            )],
        ));
        xdf.extend(samples(
            5,
            &[(Some(1.0), vec![0x80]), (Some(2.0), vec![0x7f])],
        ));
        xdf.extend(samples(
            6,
            &[(Some(1.0), le(&[i32::MIN], |v| v.to_le_bytes()))],
        ));
        let mut text = vec![1u8, 5];
        text.extend_from_slice(b"go, 1");
        xdf.extend(samples(7, &[(Some(3.5), text)]));
        // a second chunk for the double stream, cut short inside its second sample
        let mut cut = samples(
            2,
            &[
                (None, le(&[7.0f64, 8.0], |v| v.to_le_bytes())),
                (None, le(&[9.0f64, 10.0], |v| v.to_le_bytes())),
            ],
        );
        cut.truncate(cut.len() - 5);
        let fix = (cut.len() - 5) as u32;
        cut[1..5].copy_from_slice(&fix.to_le_bytes());
        xdf.extend(cut);

        let mut streams = parse_xdf_bytes(&xdf).unwrap();
        streams.sort_by_key(|s| s.stream_id);
        let by = |id: u32| streams.iter().find(|s| s.stream_id == id).unwrap();

        assert_eq!(by(1).data, XdfData::Float(vec![1.5, -2.25, 3.0, 4.0]));
        assert_eq!(by(1).timestamps, [10.0, 10.01]);
        // the complete sample of the cut chunk is kept, the torn one dropped
        let mut want = doubles.to_vec();
        want.extend([7.0, 8.0]);
        let XdfData::Float(got) = &by(2).data else {
            panic!()
        };
        assert_eq!(
            got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            want.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(by(2).timestamps.len(), 3);
        assert_eq!(by(3).data, XdfData::Int(longs.to_vec()));
        assert_eq!(by(3).timestamps, [11.0, 12.0]);
        assert_eq!(by(4).data, XdfData::Int(vec![-32768, -1, 32767]));
        assert_eq!(by(5).data, XdfData::Int(vec![-128, 127]));
        assert_eq!(by(6).data, XdfData::Int(vec![-2_147_483_648]));
        assert_eq!(by(7).data, XdfData::Text(vec!["go, 1".into()]));
        assert_eq!(by(7).data.text(0), "\"go, 1\"");
        assert_eq!(by(3).data.text(2), "9007199254740993");
    }

    /// The Parquet output keeps each stream's type, and the values read back exactly.
    #[test]
    fn parquet_columns_keep_the_stream_type() {
        let dir = std::env::temp_dir().join(format!("rlsl-convert-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let longs = [i64::MAX, (1 << 53) + 1];
        let doubles = [std::f64::consts::E, 1.0e-300];
        let mut xdf = b"XDF:".to_vec();
        xdf.extend(header(1, "Wide", 1, 0.0, "int64"));
        xdf.extend(header(2, "Fine", 1, 0.0, "double64"));
        xdf.extend(samples(
            1,
            &[
                (Some(1.0), longs[0].to_le_bytes().to_vec()),
                (Some(2.0), longs[1].to_le_bytes().to_vec()),
            ],
        ));
        xdf.extend(samples(
            2,
            &[
                (Some(1.0), doubles[0].to_le_bytes().to_vec()),
                (Some(2.0), doubles[1].to_le_bytes().to_vec()),
            ],
        ));
        let input = dir.join("in.xdf");
        std::fs::write(&input, xdf).unwrap();
        let out = dir.join("out");
        xdf_to_parquet(input.to_str().unwrap(), out.to_str().unwrap()).unwrap();

        let read = |name: &str| {
            let f = std::fs::File::open(out.join(name)).unwrap();
            let mut r = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f)
                .unwrap()
                .build()
                .unwrap();
            r.next().unwrap().unwrap()
        };
        let wide = read("stream_1_Wide.parquet");
        assert_eq!(wide.schema().field(1).data_type(), &DataType::Int64);
        assert_eq!(
            wide.column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &longs
        );
        let fine = read("stream_2_Fine.parquet");
        assert_eq!(fine.schema().field(1).data_type(), &DataType::Float64);
        assert_eq!(
            fine.column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .values(),
            &doubles
        );

        let csv = dir.join("out.csv");
        xdf_to_csv(input.to_str().unwrap(), csv.to_str().unwrap()).unwrap();
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(text.contains("Wide,2.000000000,9007199254740993"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
