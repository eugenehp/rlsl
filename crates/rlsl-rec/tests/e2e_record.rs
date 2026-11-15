//! End-to-end test: send data via outlet → record with Recording → verify output.

use arrow::array::RecordBatchReader;
use rlsl::prelude::*;
use rlsl::resolver;
use rlsl_rec::recording::{Recording, RecordingFormat};
use std::sync::atomic::Ordering;
use std::time::Duration;

// ── Helpers ──────────────────────────────────────────────────────────

async fn create_outlet(name: &str, src_id: &str) -> (StreamInfo, StreamOutlet) {
    let name = name.to_string();
    let src_id = src_id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let info = StreamInfo::new(&name, "EEG", 4, 250.0, ChannelFormat::Float32, &src_id);
        let outlet = StreamOutlet::new(&info, 0, 360);
        std::thread::sleep(Duration::from_secs(2));
        (info, outlet)
    })
    .await
    .unwrap();
    result
}

async fn resolve_stream(name: &str) -> StreamInfo {
    let resolved = tokio::task::spawn_blocking(move || resolver::resolve_all(3.0))
        .await
        .unwrap();
    resolved
        .into_iter()
        .find(|s| s.name() == name)
        .unwrap_or_else(|| panic!("Could not find stream '{}'", name))
}

fn push_samples(outlet: StreamOutlet, n: usize) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for i in 0..n {
            let sample = [
                i as f32 * 0.1,
                i as f32 * 0.2,
                i as f32 * 0.3,
                i as f32 * 0.4,
            ];
            outlet.push_sample_f(&sample, 0.0, true);
            std::thread::sleep(Duration::from_secs_f64(1.0 / 250.0));
        }
        std::thread::sleep(Duration::from_secs(2));
    })
}

// ── XDF test ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn e2e_xdf() {
    let (_info, outlet) = create_outlet("E2E_XDF", "e2e_xdf_src").await;
    let stream = resolve_stream("E2E_XDF").await;

    let path = "test_e2e_xdf.xdf";
    let rec = Recording::start_with_format(path, &[stream], RecordingFormat::Xdf)
        .expect("Failed to start XDF recording");

    let sender = push_samples(outlet, 500);
    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = rec.state.sample_count.load(Ordering::Relaxed);
    println!("[XDF] Recorded {} samples", count);
    rec.stop().await;
    sender.join().unwrap();

    // Verify XDF file
    let data = std::fs::read(path).expect("XDF file not found");
    assert_eq!(&data[0..4], b"XDF:", "Missing XDF magic");
    assert!(data.len() > 100);

    let mut pos = 4;
    let (mut has_fh, mut has_sh, mut has_samp, mut has_sf) = (false, false, false, false);
    while pos < data.len() {
        let nlb = data[pos] as usize;
        pos += 1;
        if pos + nlb > data.len() {
            break;
        }
        let length = match nlb {
            1 => data[pos] as u64,
            4 => u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as u64,
            8 => u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()),
            _ => break,
        };
        pos += nlb;
        if pos + 2 > data.len() {
            break;
        }
        let tag = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap());
        pos += length as usize;
        match tag {
            1 => has_fh = true,
            2 => has_sh = true,
            3 => has_samp = true,
            6 => has_sf = true,
            _ => {}
        }
    }
    assert!(has_fh, "Missing FileHeader");
    assert!(has_sh, "Missing StreamHeader");
    assert!(has_samp, "No sample chunks");
    assert!(has_sf, "Missing StreamFooter");
    assert!(count >= 200, "Only {} samples (expected ≥200)", count);
    println!(
        "✅ XDF e2e PASSED ({} samples, {} bytes)",
        count,
        data.len()
    );
    std::fs::remove_file(path).ok();
}

// ── Parquet test ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn e2e_parquet() {
    let (_info, outlet) = create_outlet("E2E_Parquet", "e2e_pq_src").await;
    let stream = resolve_stream("E2E_Parquet").await;

    let dir = "test_e2e_parquet";
    let rec = Recording::start_with_format(dir, &[stream], RecordingFormat::Parquet)
        .expect("Failed to start Parquet recording");

    let sender = push_samples(outlet, 500);
    tokio::time::sleep(Duration::from_secs(4)).await;

    let count = rec.state.sample_count.load(Ordering::Relaxed);
    println!("[Parquet] Recorded {} samples", count);
    rec.stop().await;
    sender.join().unwrap();

    // ── Verify directory structure ──
    assert!(
        std::path::Path::new(dir).is_dir(),
        "Output directory not created"
    );

    let entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let filenames: Vec<String> = entries
        .iter()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    println!("[Parquet] Output files: {:?}", filenames);

    assert!(
        filenames.iter().any(|f| f == "metadata.json"),
        "Missing metadata.json sidecar"
    );

    let parquet_files: Vec<&String> = filenames
        .iter()
        .filter(|f| f.ends_with(".parquet"))
        .collect();
    assert!(!parquet_files.is_empty(), "No .parquet files found");

    // ── Validate JSON sidecar ──
    let sidecar_path = std::path::Path::new(dir).join("metadata.json");
    let sidecar_str = std::fs::read_to_string(&sidecar_path).expect("Cannot read metadata.json");
    let sidecar: serde_json::Value =
        serde_json::from_str(&sidecar_str).expect("Invalid JSON sidecar");
    println!(
        "[Parquet] Sidecar:\n{}",
        serde_json::to_string_pretty(&sidecar).unwrap()
    );

    assert!(sidecar["recording_start_unix"].as_f64().unwrap() > 0.0);
    let streams = sidecar["streams"]
        .as_array()
        .expect("streams should be array");
    assert_eq!(streams.len(), 1);
    let s = &streams[0];
    assert_eq!(s["name"].as_str().unwrap(), "E2E_Parquet");
    assert_eq!(s["type"].as_str().unwrap(), "EEG");
    assert_eq!(s["channel_count"].as_u64().unwrap(), 4);
    assert!((s["nominal_srate"].as_f64().unwrap() - 250.0).abs() < 0.01);
    assert_eq!(s["channel_format"].as_str().unwrap(), "float32");
    assert!(s["sample_count"].as_u64().unwrap() > 0);
    assert!(s["first_timestamp"].as_f64().unwrap() > 0.0);
    assert!(s["last_timestamp"].as_f64().unwrap() > s["first_timestamp"].as_f64().unwrap());
    assert!(s["header_xml"]
        .as_str()
        .unwrap()
        .contains("<name>E2E_Parquet</name>"));
    assert!(s["parquet_file"].as_str().unwrap().ends_with(".parquet"));
    println!("✓ Sidecar metadata validated");

    // ── Validate Parquet file ──
    let pq_path = std::path::Path::new(dir).join(s["parquet_file"].as_str().unwrap());
    let pq_file = std::fs::File::open(&pq_path).expect("Cannot open .parquet");
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(pq_file)
        .expect("Invalid parquet file")
        .build()
        .expect("Cannot build reader");

    let schema = reader.schema();
    let field_names: Vec<&str> = schema
        .fields()
        .iter()
        .map(|f: &std::sync::Arc<arrow::datatypes::Field>| f.name().as_str())
        .collect();
    assert_eq!(field_names[0], "timestamp");
    assert_eq!(field_names.len(), 5);
    println!(
        "✓ Schema has {} columns: {:?}",
        field_names.len(),
        field_names
    );

    assert_eq!(
        *schema.field(0).data_type(),
        arrow::datatypes::DataType::Float64
    );
    for i in 1..5 {
        assert_eq!(
            *schema.field(i).data_type(),
            arrow::datatypes::DataType::Float32
        );
    }
    println!("✓ Column types correct");

    let mut total_rows = 0u64;
    let mut first_ts = f64::MAX;
    let mut last_ts = f64::MIN;
    for batch in reader {
        let batch = batch.expect("Error reading batch");
        let n = batch.num_rows();
        total_rows += n as u64;
        let ts_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("timestamp column not Float64");
        for i in 0..n {
            let t = ts_col.value(i);
            assert!(t > 0.0, "Timestamp should be positive, got {}", t);
            if t < first_ts {
                first_ts = t;
            }
            if t > last_ts {
                last_ts = t;
            }
        }
    }

    println!(
        "✓ Read {} rows, timestamps [{:.4} .. {:.4}]",
        total_rows, first_ts, last_ts
    );
    assert!(
        total_rows >= 200,
        "Only {} rows (expected ≥200)",
        total_rows
    );
    assert!(last_ts > first_ts);

    println!(
        "\n✅ Parquet e2e PASSED ({} samples recorded, {} rows in parquet)",
        count, total_rows
    );
    std::fs::remove_dir_all(dir).ok();
}

// ── 64-bit integers ──────────────────────────────────────────────────

/// An int64 stream recorded to Parquet keeps every bit: values beyond 2^53, which a double
/// cannot hold (the writer used to buffer integers as doubles and store them rounded).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_parquet_int64_is_exact() {
    const VALUES: [i64; 3] = [i64::MAX, i64::MIN, (1 << 53) + 1];
    let outlet = tokio::task::spawn_blocking(|| {
        let info = StreamInfo::new(
            "E2E_Parquet_I64",
            "Counter",
            3,
            100.0,
            ChannelFormat::Int64,
            "e2e_pq_i64",
        );
        let outlet = StreamOutlet::new(&info, 0, 360);
        std::thread::sleep(Duration::from_secs(2));
        outlet
    })
    .await
    .unwrap();
    let stream = resolve_stream("E2E_Parquet_I64").await;

    let dir = "test_e2e_parquet_i64";
    let _ = std::fs::remove_dir_all(dir);
    let rec = Recording::start_with_format(dir, &[stream], RecordingFormat::Parquet)
        .expect("Failed to start Parquet recording");
    let sender = std::thread::spawn(move || {
        // sample k carries the three values moved k towards zero
        for k in 0..200i64 {
            outlet.push_sample_i64(&[VALUES[0] - k, VALUES[1] + k, VALUES[2] + k], 0.0, true);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs(2));
    });
    tokio::time::sleep(Duration::from_secs(4)).await;
    rec.stop().await;
    sender.join().unwrap();

    let file = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "parquet"))
        .expect("a .parquet file");
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(&file).unwrap(),
    )
    .unwrap()
    .build()
    .unwrap();
    let mut rows = 0;
    for batch in reader {
        let batch = batch.unwrap();
        let col = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::Int64Array>()
                .expect("an Int64 column")
                .clone()
        };
        let (a, b, c) = (col(1), col(2), col(3));
        for i in 0..batch.num_rows() {
            // which sample this is, from the first channel; the others must agree exactly
            let k = VALUES[0] - a.value(i);
            assert!((0..200).contains(&k), "channel 1 holds {}", a.value(i));
            assert_eq!(b.value(i), VALUES[1] + k, "row {rows}");
            assert_eq!(c.value(i), VALUES[2] + k, "row {rows}");
            rows += 1;
        }
    }
    assert!(rows >= 100, "only {rows} rows recorded");
    std::fs::remove_dir_all(dir).ok();
}

// ── Clock offsets ────────────────────────────────────────────────────

/// A time service whose clock is `ahead` seconds ahead of this machine's.
fn fake_time_service(ahead: f64) -> u16 {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let mut lines = text.lines();
            if lines.next().map(str::trim) != Some("LSL:timedata") {
                continue;
            }
            let params: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
            let remote_now = rlsl::clock::local_clock() + ahead;
            let reply = format!(
                " {} {} {remote_now} {remote_now}",
                params.first().unwrap_or(&"0"),
                params.get(1).unwrap_or(&"0")
            );
            let _ = socket.send_to(reply.as_bytes(), from);
        }
    });
    port
}

/// The clock offset written to XDF is the value a reader adds to the stream's time stamps: for
/// an outlet whose clock is 100 s ahead of the recorder's, -100. (It used to be written with the
/// opposite sign.)
#[tokio::test(flavor = "multi_thread")]
async fn e2e_xdf_clock_offset_is_what_readers_add() {
    let (_info, outlet) = create_outlet("E2E_Offset", "e2e_offset_src").await;
    let stream = resolve_stream("E2E_Offset").await;
    stream.set_v4address("127.0.0.1");
    stream.set_v4service_port(fake_time_service(100.0));

    let path = "test_e2e_offset.xdf";
    let rec = Recording::start_with_format(path, &[stream], RecordingFormat::Xdf).expect("start");
    let sender = push_samples(outlet, 1500);
    tokio::time::sleep(Duration::from_secs(8)).await;
    rec.stop().await;
    sender.join().unwrap();

    // ClockOffset chunks (tag 4): stream id, collection time, offset
    let data = std::fs::read(path).unwrap();
    let (mut pos, mut offsets) = (4, Vec::new());
    while pos < data.len() {
        let nlb = data[pos] as usize;
        pos += 1;
        let len = data[pos..pos + nlb]
            .iter()
            .enumerate()
            .fold(0u64, |a, (i, b)| a | (*b as u64) << (8 * i)) as usize;
        pos += nlb;
        if u16::from_le_bytes([data[pos], data[pos + 1]]) == 4 {
            offsets.push(f64::from_le_bytes(
                data[pos + 14..pos + 22].try_into().unwrap(),
            ));
        }
        pos += len;
    }
    std::fs::remove_file(path).ok();
    assert!(!offsets.is_empty(), "no clock offset was recorded");
    assert!(
        offsets.iter().all(|o| (o + 100.0).abs() < 0.05),
        "{offsets:?}"
    );
}

/// An 8-bit integer stream is recorded (it used to be skipped: nothing pulled it).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_parquet_int8_is_recorded() {
    let outlet = tokio::task::spawn_blocking(|| {
        let info = StreamInfo::new(
            "E2E_Parquet_I8",
            "Codes",
            2,
            50.0,
            ChannelFormat::Int8,
            "e2e_pq_i8",
        );
        let outlet = StreamOutlet::new(&info, 0, 360);
        std::thread::sleep(Duration::from_secs(2));
        outlet
    })
    .await
    .unwrap();
    let stream = resolve_stream("E2E_Parquet_I8").await;
    let dir = "test_e2e_parquet_i8";
    let _ = std::fs::remove_dir_all(dir);
    let rec =
        Recording::start_with_format(dir, &[stream], RecordingFormat::Parquet).expect("start");
    let sender = std::thread::spawn(move || {
        for k in 0..150i32 {
            outlet.push_sample_i8(&[(k % 100) as i8, -((k % 100) as i8)], 0.0, true);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    tokio::time::sleep(Duration::from_secs(4)).await;
    rec.stop().await;
    sender.join().unwrap();
    let file = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "parquet"))
        .expect("a .parquet file");
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(&file).unwrap(),
    )
    .unwrap()
    .build()
    .unwrap();
    let mut rows = 0;
    for batch in reader {
        let batch = batch.unwrap();
        let col = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::Int8Array>()
                .expect("an Int8 column")
                .clone()
        };
        let (a, b) = (col(1), col(2));
        for i in 0..batch.num_rows() {
            assert!((0..100).contains(&a.value(i)));
            assert_eq!(b.value(i), -a.value(i));
            rows += 1;
        }
    }
    assert!(rows >= 80, "only {rows} rows recorded");
    std::fs::remove_dir_all(dir).ok();
}

// ── String streams (markers) ─────────────────────────────────────────

/// A marker stream — strings, no rate — is recorded, in every format. (Up to 0.0.8 a string
/// stream was opened and nothing of it written.)
#[tokio::test(flavor = "multi_thread")]
async fn e2e_markers_are_recorded_in_every_format() {
    #[cfg_attr(not(feature = "hdf5"), allow(unused_mut))]
    let mut formats = vec![
        (RecordingFormat::Xdf, "test_e2e_markers.xdf"),
        (RecordingFormat::Parquet, "test_e2e_markers_parquet"),
    ];
    #[cfg(feature = "hdf5")]
    formats.push((RecordingFormat::Hdf5, "test_e2e_markers.h5"));

    for (k, (format, path)) in formats.into_iter().enumerate() {
        let name = format!("E2E_Markers_{k}");
        let outlet_name = name.clone();
        let outlet = tokio::task::spawn_blocking(move || {
            let info = StreamInfo::new(
                &outlet_name,
                "Markers",
                1,
                0.0,
                ChannelFormat::String,
                &outlet_name,
            );
            let outlet = StreamOutlet::new(&info, 0, 360);
            std::thread::sleep(Duration::from_secs(2));
            outlet
        })
        .await
        .unwrap();
        let stream = resolve_stream(&name).await;
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
        let rec = Recording::start_with_format(path, &[stream], format).expect("start");
        let sender = std::thread::spawn(move || {
            while !outlet.have_consumers() {
                std::thread::sleep(Duration::from_millis(20));
            }
            for i in 0..40 {
                outlet.push_sample_str(
                    &[format!("marker-{i} \u{e9}")],
                    1000.0 + f64::from(i),
                    true,
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            std::thread::sleep(Duration::from_secs(1));
        });
        tokio::time::sleep(Duration::from_secs(4)).await;
        let recorded = rec.state.sample_count.load(Ordering::Relaxed);
        rec.stop().await;
        sender.join().unwrap();
        assert_eq!(recorded, 40, "{format:?}: samples counted");

        let want: Vec<String> = (0..40).map(|i| format!("marker-{i} \u{e9}")).collect();
        match format {
            RecordingFormat::Xdf => {
                let bytes = std::fs::read(path).unwrap();
                let has =
                    |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
                assert!(
                    want.iter().all(|m| has(m)),
                    "a marker is missing from the XDF file"
                );
                if std::env::var_os("RLSL_KEEP_TEST_FILES").is_none() {
                    let _ = std::fs::remove_file(path);
                }
            }
            RecordingFormat::Parquet => {
                let file = std::fs::read_dir(path)
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|x| x == "parquet"))
                    .expect("a .parquet file");
                let reader =
                    parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                        std::fs::File::open(&file).unwrap(),
                    )
                    .unwrap()
                    .build()
                    .unwrap();
                let (mut stamps, mut texts) = (Vec::new(), Vec::new());
                for batch in reader {
                    let batch = batch.unwrap();
                    let ts = batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<arrow::array::Float64Array>()
                        .unwrap()
                        .clone();
                    let text = batch
                        .column(1)
                        .as_any()
                        .downcast_ref::<arrow::array::StringArray>()
                        .expect("a string column")
                        .clone();
                    for i in 0..batch.num_rows() {
                        stamps.push(ts.value(i));
                        texts.push(text.value(i).to_string());
                    }
                }
                assert_eq!(texts, want);
                assert!(stamps
                    .iter()
                    .enumerate()
                    .all(|(i, t)| (*t - (1000.0 + i as f64)).abs() < 1e-6));
                let _ = std::fs::remove_dir_all(path);
            }
            #[cfg(feature = "hdf5")]
            RecordingFormat::Hdf5 => {
                let file = hdf5::File::open(path).unwrap();
                let group = file.group("stream_1_Markers").unwrap();
                let texts: Vec<hdf5::types::VarLenUnicode> =
                    group.dataset("data").unwrap().read_raw().unwrap();
                let texts: Vec<String> = texts.iter().map(|t| t.as_str().to_string()).collect();
                assert_eq!(texts, want);
                let stamps: Vec<f64> = group.dataset("timestamps").unwrap().read_raw().unwrap();
                assert!(stamps
                    .iter()
                    .enumerate()
                    .all(|(i, t)| (*t - (1000.0 + i as f64)).abs() < 1e-6));
                let count: u64 = group.attr("sample_count").unwrap().read_scalar().unwrap();
                assert_eq!(count, 40);
                drop(file);
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

// ── HDF5 ─────────────────────────────────────────────────────────────

/// Numeric streams in an HDF5 recording: the stream's own type, every value, the description.
#[cfg(feature = "hdf5")]
#[tokio::test(flavor = "multi_thread")]
async fn e2e_hdf5_keeps_types_values_and_description() {
    let outlets = tokio::task::spawn_blocking(|| {
        let eeg = StreamInfo::new(
            "E2E_H5_EEG",
            "EEG",
            3,
            100.0,
            ChannelFormat::Float32,
            "e2e_h5_eeg",
        );
        let channels = eeg.desc().append_child("channels");
        for label in ["Fp1", "Cz", "O2"] {
            channels
                .append_child("channel")
                .append_child_value("label", label);
        }
        let counts = StreamInfo::new(
            "E2E_H5_Counts",
            "Counts",
            2,
            100.0,
            ChannelFormat::Int64,
            "e2e_h5_counts",
        );
        let outlets = (
            StreamOutlet::new(&eeg, 0, 360),
            StreamOutlet::new(&counts, 0, 360),
        );
        std::thread::sleep(Duration::from_secs(2));
        outlets
    })
    .await
    .unwrap();
    let streams = vec![
        resolve_stream("E2E_H5_EEG").await,
        resolve_stream("E2E_H5_Counts").await,
    ];
    let path = "test_e2e_types.h5";
    let _ = std::fs::remove_file(path);
    let rec = Recording::start_with_format(path, &streams, RecordingFormat::Hdf5).expect("start");
    const BIG: i64 = (1 << 53) + 1; // not a double
    let sender = std::thread::spawn(move || {
        let (eeg, counts) = outlets;
        while !(eeg.have_consumers() && counts.have_consumers()) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // more than one block of the writer, so that the file grows while recording
        for k in 0..2500i64 {
            let t = 2000.0 + k as f64 * 0.01;
            eeg.push_sample_f(&[k as f32 * 0.5, -(k as f32), 1.0e-3], t, true);
            counts.push_sample_i64(&[BIG + k, -k], t, true);
            if k % 100 == 99 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    tokio::time::sleep(Duration::from_secs(5)).await;
    rec.stop().await;
    sender.join().unwrap();

    let file = hdf5::File::open(path).unwrap();
    let eeg = file.group("stream_1_EEG").unwrap();
    let data: ndarray::Array2<f32> = eeg.dataset("data").unwrap().read_2d().unwrap();
    let stamps: Vec<f64> = eeg.dataset("timestamps").unwrap().read_raw().unwrap();
    assert_eq!((data.nrows(), data.ncols(), stamps.len()), (2500, 3, 2500));
    for k in 0..2500 {
        assert_eq!(
            (data[[k, 0]], data[[k, 1]], data[[k, 2]]),
            (k as f32 * 0.5, -(k as f32), 1.0e-3),
            "row {k}"
        );
        assert!(
            (stamps[k] - (2000.0 + k as f64 * 0.01)).abs() < 1e-6,
            "row {k}: {}",
            stamps[k]
        );
    }
    let labels: Vec<hdf5::types::VarLenUnicode> =
        eeg.dataset("channel_labels").unwrap().read_raw().unwrap();
    assert_eq!(
        labels.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
        ["Fp1", "Cz", "O2"]
    );
    let name: hdf5::types::VarLenUnicode = eeg.attr("name").unwrap().read_scalar().unwrap();
    let rate: f64 = eeg.attr("nominal_srate").unwrap().read_scalar().unwrap();
    let count: u64 = eeg.attr("sample_count").unwrap().read_scalar().unwrap();
    assert_eq!((name.as_str(), rate, count), ("E2E_H5_EEG", 100.0, 2500));

    let counts = file.group("stream_2_Counts").unwrap();
    let data: ndarray::Array2<i64> = counts.dataset("data").unwrap().read_2d().unwrap();
    assert_eq!(data.nrows(), 2500);
    for k in 0..2500 {
        assert_eq!(
            (data[[k, 0]], data[[k, 1]]),
            (BIG + k as i64, -(k as i64)),
            "row {k}"
        );
    }
    let bytes = std::fs::metadata(path).unwrap().len();
    eprintln!("2500 x (3 float32 + 2 int64) + time stamps in HDF5: {bytes} bytes");
    drop(file);
    // (kept on request, to look at with another HDF5 reader)
    if std::env::var_os("RLSL_KEEP_TEST_FILES").is_none() {
        let _ = std::fs::remove_file(path);
    }
}
