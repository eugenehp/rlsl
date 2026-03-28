//! Comprehensive unit tests for lsl-core modules.

// ── clock tests ──────────────────────────────────────────────────────

mod clock_tests {
    use lsl_core::clock::local_clock;

    #[test]
    fn clock_returns_positive() {
        let t = local_clock();
        assert!(
            t > 0.0,
            "local_clock() should return positive value, got {}",
            t
        );
    }

    #[test]
    fn clock_is_monotonic() {
        let t1 = local_clock();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = local_clock();
        assert!(t2 > t1, "local_clock() should be monotonically increasing");
    }

    #[test]
    fn clock_resolution_sub_millisecond() {
        // Measure the smallest observable increment
        let mut min_delta = f64::MAX;
        for _ in 0..1000 {
            let t1 = local_clock();
            let t2 = local_clock();
            if t2 > t1 {
                min_delta = min_delta.min(t2 - t1);
            }
        }
        // Should be able to resolve at least 1ms differences
        assert!(
            min_delta < 0.001,
            "Clock resolution should be < 1ms, got {}s",
            min_delta
        );
    }
}

// ── types tests ──────────────────────────────────────────────────────

mod types_tests {
    use lsl_core::types::*;

    #[test]
    fn channel_format_from_i32_roundtrip() {
        for fmt_val in 0..=7 {
            let fmt = ChannelFormat::from_i32(fmt_val);
            if fmt_val == 0 {
                assert_eq!(fmt, ChannelFormat::Undefined);
            } else {
                assert_eq!(fmt as i32, fmt_val);
            }
        }
    }

    #[test]
    fn channel_format_from_str_roundtrip() {
        let formats = [
            ("float32", ChannelFormat::Float32),
            ("double64", ChannelFormat::Double64),
            ("string", ChannelFormat::String),
            ("int32", ChannelFormat::Int32),
            ("int16", ChannelFormat::Int16),
            ("int8", ChannelFormat::Int8),
            ("int64", ChannelFormat::Int64),
        ];
        for (s, expected) in &formats {
            let fmt = ChannelFormat::from_name(s);
            assert_eq!(fmt, *expected, "from_str({}) failed", s);
            assert_eq!(fmt.as_str(), *s, "as_str() roundtrip failed for {}", s);
        }
    }

    #[test]
    fn channel_format_unknown_str() {
        assert_eq!(ChannelFormat::from_name("bogus"), ChannelFormat::Undefined);
        assert_eq!(ChannelFormat::from_i32(999), ChannelFormat::Undefined);
    }

    #[test]
    fn channel_bytes() {
        assert_eq!(ChannelFormat::Float32.channel_bytes(), 4);
        assert_eq!(ChannelFormat::Double64.channel_bytes(), 8);
        assert_eq!(ChannelFormat::Int32.channel_bytes(), 4);
        assert_eq!(ChannelFormat::Int16.channel_bytes(), 2);
        assert_eq!(ChannelFormat::Int8.channel_bytes(), 1);
        assert_eq!(ChannelFormat::Int64.channel_bytes(), 8);
        assert_eq!(ChannelFormat::String.channel_bytes(), 0);
        assert_eq!(ChannelFormat::Undefined.channel_bytes(), 0);
    }

    #[test]
    fn constants_match_liblsl() {
        assert_eq!(LSL_PROTOCOL_VERSION, 110);
        assert_eq!(IRREGULAR_RATE, 0.0);
        assert_eq!(DEDUCED_TIMESTAMP, -1.0);
        assert_eq!(MULTICAST_PORT, 16571);
        assert_eq!(BASE_PORT, 16572);
        assert_eq!(PORT_RANGE, 32);
        assert_eq!(TAG_DEDUCED_TIMESTAMP, 1);
        assert_eq!(TAG_TRANSMITTED_TIMESTAMP, 2);
    }

    #[test]
    fn proc_flags() {
        assert_eq!(PROC_NONE, 0);
        assert_eq!(
            PROC_ALL,
            PROC_CLOCKSYNC | PROC_DEJITTER | PROC_MONOTONIZE | PROC_THREADSAFE
        );
    }
}

// ── sample tests ─────────────────────────────────────────────────────

mod sample_tests {
    use lsl_core::sample::*;
    use lsl_core::types::*;
    use std::io::Cursor;

    #[test]
    fn sample_new_float32() {
        let s = Sample::new(ChannelFormat::Float32, 4, 1.0);
        assert_eq!(s.num_channels(), 4);
        assert_eq!(s.format(), ChannelFormat::Float32);
        assert_eq!(s.timestamp, 1.0);
        assert!(s.pushthrough);
    }

    #[test]
    fn sample_new_all_formats() {
        let formats = [
            ChannelFormat::Float32,
            ChannelFormat::Double64,
            ChannelFormat::Int32,
            ChannelFormat::Int16,
            ChannelFormat::Int8,
            ChannelFormat::Int64,
            ChannelFormat::String,
        ];
        for fmt in &formats {
            let s = Sample::new(*fmt, 3, 0.0);
            assert_eq!(s.num_channels(), 3);
            assert_eq!(s.format(), *fmt);
        }
    }

    #[test]
    fn assign_retrieve_f32() {
        let mut s = Sample::new(ChannelFormat::Float32, 3, 0.0);
        s.assign_f32(&[1.5, 2.5, 3.5]);
        let mut out = [0.0f32; 3];
        s.retrieve_f32(&mut out);
        assert_eq!(out, [1.5, 2.5, 3.5]);
    }

    #[test]
    fn assign_retrieve_f64() {
        let mut s = Sample::new(ChannelFormat::Double64, 2, 0.0);
        s.assign_f64(&[1e15, -1e15]);
        let mut out = [0.0f64; 2];
        s.retrieve_f64(&mut out);
        assert_eq!(out, [1e15, -1e15]);
    }

    #[test]
    fn assign_retrieve_i32() {
        let mut s = Sample::new(ChannelFormat::Int32, 2, 0.0);
        s.assign_i32(&[i32::MAX, i32::MIN]);
        let mut out = [0i32; 2];
        s.retrieve_i32(&mut out);
        assert_eq!(out, [i32::MAX, i32::MIN]);
    }

    #[test]
    fn assign_retrieve_i16() {
        let mut s = Sample::new(ChannelFormat::Int16, 2, 0.0);
        s.assign_i16(&[i16::MAX, i16::MIN]);
        let mut out = [0i16; 2];
        s.retrieve_i16(&mut out);
        assert_eq!(out, [i16::MAX, i16::MIN]);
    }

    #[test]
    fn assign_retrieve_i8() {
        let mut s = Sample::new(ChannelFormat::Int8, 2, 0.0);
        s.assign_i8(&[i8::MAX, i8::MIN]);
        let mut out = [0i8; 2];
        s.retrieve_i8(&mut out);
        assert_eq!(out, [i8::MAX, i8::MIN]);
    }

    #[test]
    fn assign_retrieve_i64() {
        let mut s = Sample::new(ChannelFormat::Int64, 2, 0.0);
        s.assign_i64(&[i64::MAX, i64::MIN]);
        let mut out = [0i64; 2];
        s.retrieve_i64(&mut out);
        assert_eq!(out, [i64::MAX, i64::MIN]);
    }

    #[test]
    fn assign_retrieve_strings() {
        let mut s = Sample::new(ChannelFormat::String, 3, 0.0);
        s.assign_strings(&["hello".to_string(), "world".to_string(), "".to_string()]);
        let out = s.retrieve_strings();
        assert_eq!(out, vec!["hello", "world", ""]);
    }

    #[test]
    fn cross_format_f32_to_i32() {
        let mut s = Sample::new(ChannelFormat::Int32, 2, 0.0);
        s.assign_f32(&[42.7, -10.3]);
        let mut out = [0i32; 2];
        s.retrieve_i32(&mut out);
        assert_eq!(out, [42, -10]);
    }

    #[test]
    fn cross_format_i32_to_f64() {
        let mut s = Sample::new(ChannelFormat::Float32, 2, 0.0);
        s.assign_i32(&[100, -200]);
        let mut out = [0.0f64; 2];
        s.retrieve_f64(&mut out);
        assert_eq!(out, [100.0, -200.0]);
    }

    #[test]
    fn raw_bytes_roundtrip_float32() {
        let mut s = Sample::new(ChannelFormat::Float32, 3, 0.0);
        s.assign_f32(&[1.0, 2.0, 3.0]);
        let raw = s.retrieve_raw();
        assert_eq!(raw.len(), 12); // 3 × 4 bytes
        let mut s2 = Sample::new(ChannelFormat::Float32, 3, 0.0);
        s2.assign_raw(&raw);
        let mut out = [0.0f32; 3];
        s2.retrieve_f32(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn raw_bytes_roundtrip_int8() {
        let mut s = Sample::new(ChannelFormat::Int8, 4, 0.0);
        s.assign_i8(&[1, -2, 3, -4]);
        let raw = s.retrieve_raw();
        assert_eq!(raw.len(), 4);
        let mut s2 = Sample::new(ChannelFormat::Int8, 4, 0.0);
        s2.assign_raw(&raw);
        let mut out = [0i8; 4];
        s2.retrieve_i8(&mut out);
        assert_eq!(out, [1, -2, 3, -4]);
    }

    #[test]
    fn serialize_deserialize_110_float32() {
        let mut s = Sample::new(ChannelFormat::Float32, 4, 0.0);
        s.timestamp = 42.0;
        s.assign_f32(&[1.0, 2.0, 3.0, 4.0]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);

        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::Float32, 4).unwrap();
        assert_eq!(s, decoded);
    }

    #[test]
    fn serialize_deserialize_110_deduced_timestamp() {
        let mut s = Sample::new(ChannelFormat::Float32, 2, DEDUCED_TIMESTAMP);
        s.assign_f32(&[10.0, 20.0]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);
        // Deduced: 1 byte tag + 8 bytes data = 9 bytes (no timestamp)
        assert_eq!(buf.len(), 1 + 8);

        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::Float32, 2).unwrap();
        assert_eq!(decoded.timestamp, DEDUCED_TIMESTAMP);
    }

    #[test]
    fn serialize_deserialize_110_transmitted_timestamp() {
        let mut s = Sample::new(ChannelFormat::Float32, 2, 0.0);
        s.timestamp = 99.99;
        s.assign_f32(&[10.0, 20.0]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);
        // Transmitted: 1 byte tag + 8 bytes timestamp + 8 bytes data = 17 bytes
        assert_eq!(buf.len(), 1 + 8 + 8);
    }

    #[test]
    fn serialize_deserialize_110_all_numeric_formats() {
        let formats_and_bytes: Vec<(ChannelFormat, usize)> = vec![
            (ChannelFormat::Float32, 4),
            (ChannelFormat::Double64, 8),
            (ChannelFormat::Int32, 4),
            (ChannelFormat::Int16, 2),
            (ChannelFormat::Int8, 1),
            (ChannelFormat::Int64, 8),
        ];

        for (fmt, _) in &formats_and_bytes {
            let mut s = Sample::new(*fmt, 3, 0.0);
            s.assign_test_pattern(7);

            let mut buf = Vec::new();
            s.serialize_110(&mut buf);
            let mut cursor = Cursor::new(&buf);
            let decoded = Sample::deserialize_110(&mut cursor, *fmt, 3).unwrap();
            assert_eq!(s, decoded, "Roundtrip failed for {:?}", fmt);
        }
    }

    #[test]
    fn serialize_deserialize_110_strings() {
        let mut s = Sample::new(ChannelFormat::String, 2, 0.0);
        s.timestamp = 1.0;
        s.assign_strings(&["hello".to_string(), "world".to_string()]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);
        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::String, 2).unwrap();
        assert_eq!(decoded.retrieve_strings(), vec!["hello", "world"]);
    }

    #[test]
    fn serialize_deserialize_110_long_string() {
        let long = "x".repeat(300); // longer than 255, triggers u32 length
        let mut s = Sample::new(ChannelFormat::String, 1, 0.0);
        s.timestamp = 0.0;
        s.assign_strings(&[long.clone()]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);
        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::String, 1).unwrap();
        assert_eq!(decoded.retrieve_strings(), vec![long]);
    }

    #[test]
    fn serialize_deserialize_110_empty_string() {
        let mut s = Sample::new(ChannelFormat::String, 2, 0.0);
        s.timestamp = 0.0;
        s.assign_strings(&["".to_string(), "notempty".to_string()]);

        let mut buf = Vec::new();
        s.serialize_110(&mut buf);
        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::String, 2).unwrap();
        assert_eq!(decoded.retrieve_strings(), vec!["", "notempty"]);
    }

    #[test]
    fn serialize_deserialize_100_roundtrip() {
        let formats = [
            ChannelFormat::Float32,
            ChannelFormat::Double64,
            ChannelFormat::Int32,
            ChannelFormat::Int16,
            ChannelFormat::Int8,
            ChannelFormat::Int64,
            ChannelFormat::String,
        ];

        for fmt in &formats {
            let mut s = Sample::new(*fmt, 3, 0.0);
            s.assign_test_pattern(0);

            let mut buf = Vec::new();
            s.serialize_100(&mut buf);
            let mut cursor = Cursor::new(&buf);
            let decoded = Sample::deserialize_100(&mut cursor, *fmt, 3).unwrap();
            assert_eq!(s, decoded, "Protocol 1.00 roundtrip failed for {:?}", fmt);
        }
    }

    #[test]
    fn test_pattern_deterministic() {
        let mut s1 = Sample::new(ChannelFormat::Float32, 4, 0.0);
        s1.assign_test_pattern(5);
        let mut s2 = Sample::new(ChannelFormat::Float32, 4, 0.0);
        s2.assign_test_pattern(5);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_pattern_different_offsets() {
        let mut s1 = Sample::new(ChannelFormat::Float32, 4, 0.0);
        s1.assign_test_pattern(0);
        let mut s2 = Sample::new(ChannelFormat::Float32, 4, 0.0);
        s2.assign_test_pattern(1);
        assert_ne!(s1, s2);
    }

    #[test]
    fn sample_equality() {
        let mut s1 = Sample::new(ChannelFormat::Float32, 2, 1.0);
        s1.assign_f32(&[1.0, 2.0]);
        let mut s2 = Sample::new(ChannelFormat::Float32, 2, 1.0);
        s2.assign_f32(&[1.0, 2.0]);
        assert_eq!(s1, s2);

        s2.timestamp = 2.0;
        assert_ne!(s1, s2);
    }

    #[test]
    fn sample_equality_different_formats() {
        let s1 = Sample::new(ChannelFormat::Float32, 2, 0.0);
        let s2 = Sample::new(ChannelFormat::Int32, 2, 0.0);
        assert_ne!(s1, s2);
    }

    #[test]
    fn deserialize_110_invalid_varlen() {
        // A string sample with invalid length-size byte
        let data: Vec<u8> = vec![
            TAG_TRANSMITTED_TIMESTAMP,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // timestamp
            7, // invalid len_size (not 1, 4, or 8)
        ];
        let mut cursor = Cursor::new(&data);
        let result = Sample::deserialize_110(&mut cursor, ChannelFormat::String, 1);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_110_truncated_data() {
        // Too few bytes for a float32 sample with 4 channels
        let data: Vec<u8> = vec![
            TAG_TRANSMITTED_TIMESTAMP,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // timestamp
            0,
            0,
            0,
            0, // only 4 bytes instead of 16
        ];
        let mut cursor = Cursor::new(&data);
        let result = Sample::deserialize_110(&mut cursor, ChannelFormat::Float32, 4);
        assert!(result.is_err());
    }

    #[test]
    fn multiple_samples_sequential_110() {
        let mut buf = Vec::new();
        for i in 0..10 {
            let mut s = Sample::new(ChannelFormat::Float32, 2, 0.0);
            s.timestamp = i as f64;
            s.assign_f32(&[i as f32, (i * 10) as f32]);
            s.serialize_110(&mut buf);
        }

        let mut cursor = Cursor::new(&buf);
        for i in 0..10 {
            let decoded = Sample::deserialize_110(&mut cursor, ChannelFormat::Float32, 2).unwrap();
            assert_eq!(decoded.timestamp, i as f64);
            let mut out = [0.0f32; 2];
            decoded.retrieve_f32(&mut out);
            assert_eq!(out, [i as f32, (i * 10) as f32]);
        }
    }
}

// ── xml_dom tests ────────────────────────────────────────────────────

mod xml_dom_tests {
    use lsl_core::xml_dom::*;

    #[test]
    fn empty_node() {
        let node = XmlNode::empty();
        assert!(node.is_empty());
        assert_eq!(node.name(), "");
    }

    #[test]
    fn create_named_node() {
        let node = XmlNode::new("test");
        assert!(!node.is_empty());
        assert_eq!(node.name(), "test");
        assert_eq!(node.value(), "");
    }

    #[test]
    fn set_name_value() {
        let node = XmlNode::new("old");
        node.set_name("new");
        assert_eq!(node.name(), "new");
        node.set_value("hello");
        assert_eq!(node.value(), "hello");
    }

    #[test]
    fn append_child() {
        let root = XmlNode::new("root");
        let child = root.append_child("child");
        assert_eq!(child.name(), "child");
        assert_eq!(root.first_child().name(), "child");
        assert_eq!(root.last_child().name(), "child");
    }

    #[test]
    fn prepend_child() {
        let root = XmlNode::new("root");
        root.append_child("second");
        root.prepend_child("first");
        assert_eq!(root.first_child().name(), "first");
        assert_eq!(root.last_child().name(), "second");
    }

    #[test]
    fn append_child_value() {
        let root = XmlNode::new("root");
        root.append_child_value("name", "hello");
        let child = root.child("name");
        assert!(!child.is_empty());
        assert_eq!(child.value(), "hello");
    }

    #[test]
    fn child_value_shortcut() {
        let root = XmlNode::new("root");
        root.append_child_value("label", "C3");
        assert_eq!(root.child_value("label"), "C3");
        assert_eq!(root.child_value("nonexistent"), "");
    }

    #[test]
    fn set_child_value_creates_if_missing() {
        let root = XmlNode::new("root");
        root.set_child_value("key", "value1");
        assert_eq!(root.child_value("key"), "value1");

        // Update existing
        root.set_child_value("key", "value2");
        assert_eq!(root.child_value("key"), "value2");
    }

    #[test]
    fn child_not_found() {
        let root = XmlNode::new("root");
        let child = root.child("nonexistent");
        assert!(child.is_empty());
    }

    #[test]
    fn next_previous_sibling() {
        let root = XmlNode::new("root");
        let a = root.append_child("a");
        let b = root.append_child("b");
        let c = root.append_child("c");

        assert_eq!(a.next_sibling().name(), "b");
        assert_eq!(b.next_sibling().name(), "c");
        assert!(c.next_sibling().is_empty());

        assert!(a.previous_sibling().is_empty());
        assert_eq!(b.previous_sibling().name(), "a");
        assert_eq!(c.previous_sibling().name(), "b");
    }

    #[test]
    fn next_sibling_named() {
        let root = XmlNode::new("root");
        let ch1 = root.append_child("channel");
        root.append_child("other");
        let ch2 = root.append_child("channel");

        let next = ch1.next_sibling_named("channel");
        assert!(next.same_as(&ch2));
    }

    #[test]
    fn parent_navigation() {
        let root = XmlNode::new("root");
        let child = root.append_child("child");
        let grandchild = child.append_child("grandchild");

        assert!(grandchild.parent().same_as(&child));
        assert!(child.parent().same_as(&root));
    }

    #[test]
    fn remove_child_named() {
        let root = XmlNode::new("root");
        root.append_child("keep");
        root.append_child("remove");
        root.append_child("keep2");

        root.remove_child_named("remove");
        assert_eq!(root.first_child().name(), "keep");
        assert_eq!(root.last_child().name(), "keep2");
        assert!(root.child("remove").is_empty());
    }

    #[test]
    fn remove_specific_child() {
        let root = XmlNode::new("root");
        root.append_child("a");
        let b = root.append_child("b");
        root.append_child("c");

        root.remove_child(&b);
        assert_eq!(root.first_child().name(), "a");
        assert_eq!(root.first_child().next_sibling().name(), "c");
    }

    #[test]
    fn deep_clone() {
        let root = XmlNode::new("root");
        root.set_value("rv");
        let child = root.append_child("child");
        child.set_value("cv");

        let cloned = root.deep_clone();
        assert!(!cloned.same_as(&root));
        assert_eq!(cloned.name(), "root");
        assert_eq!(cloned.value(), "rv");
        assert_eq!(cloned.first_child().name(), "child");
        assert_eq!(cloned.first_child().value(), "cv");

        // Modifying clone doesn't affect original
        cloned.set_value("modified");
        assert_eq!(root.value(), "rv");
    }

    #[test]
    fn append_copy() {
        let root = XmlNode::new("root");
        let src = XmlNode::new("src");
        src.append_child_value("x", "1");

        let copy = root.append_copy(&src);
        assert_eq!(copy.name(), "src");
        assert_eq!(copy.child_value("x"), "1");
        assert!(!copy.same_as(&src));
    }

    #[test]
    fn to_xml_simple() {
        let root = XmlNode::new("root");
        root.append_child_value("name", "test");
        let xml = root.to_xml();
        assert_eq!(xml, "<root><name>test</name></root>");
    }

    #[test]
    fn to_xml_nested() {
        let root = XmlNode::new("desc");
        let channels = root.append_child("channels");
        let ch = channels.append_child("channel");
        ch.append_child_value("label", "C3");
        ch.append_child_value("unit", "uV");

        let xml = root.to_xml();
        assert!(xml.contains("<channels>"));
        assert!(xml.contains("<label>C3</label>"));
        assert!(xml.contains("<unit>uV</unit>"));
        assert!(xml.contains("</channels>"));
    }

    #[test]
    fn to_xml_escaping() {
        let root = XmlNode::new("root");
        root.append_child_value("val", "a<b>c&d\"e'f");
        let xml = root.to_xml();
        assert!(xml.contains("a&lt;b&gt;c&amp;d&quot;e&apos;f"));
    }

    #[test]
    fn xml_escape_unescape_roundtrip() {
        let original = "hello <world> & \"quotes\" 'apos'";
        let escaped = xml_escape(original);
        let unescaped = xml_unescape(&escaped);
        assert_eq!(unescaped, original);
    }

    #[test]
    fn xml_escape_no_special_chars() {
        assert_eq!(xml_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn same_as_identity() {
        let a = XmlNode::new("a");
        let b = a.clone();
        assert!(a.same_as(&b)); // clone shares Arc
    }

    #[test]
    fn same_as_different_nodes() {
        let a = XmlNode::new("a");
        let b = XmlNode::new("a");
        assert!(!a.same_as(&b));
    }

    #[test]
    fn empty_node_is_shared_singleton() {
        let e1 = XmlNode::empty();
        let e2 = XmlNode::empty();
        assert!(e1.same_as(&e2));
    }

    #[test]
    fn complex_tree_navigation() {
        let root = XmlNode::new("desc");
        let channels = root.append_child("channels");
        for i in 0..5 {
            let ch = channels.append_child("channel");
            ch.append_child_value("label", &format!("Ch{}", i));
            ch.append_child_value("unit", "uV");
            ch.append_child_value("type", "EEG");
        }

        // Navigate forward through all channels
        let mut ch = channels.first_child();
        let mut labels = Vec::new();
        while !ch.is_empty() {
            labels.push(ch.child_value("label"));
            ch = ch.next_sibling();
        }
        assert_eq!(labels, vec!["Ch0", "Ch1", "Ch2", "Ch3", "Ch4"]);

        // Navigate backward
        let mut ch = channels.last_child();
        let mut rev_labels = Vec::new();
        while !ch.is_empty() {
            rev_labels.push(ch.child_value("label"));
            ch = ch.previous_sibling();
        }
        rev_labels.reverse();
        assert_eq!(labels, rev_labels);
    }
}

// ── stream_info tests ────────────────────────────────────────────────

mod stream_info_tests {
    use lsl_core::stream_info::StreamInfo;
    use lsl_core::types::*;

    #[test]
    fn new_stream_info() {
        let info = StreamInfo::new("Test", "EEG", 8, 250.0, ChannelFormat::Float32, "src1");
        assert_eq!(info.name(), "Test");
        assert_eq!(info.type_(), "EEG");
        assert_eq!(info.channel_count(), 8);
        assert_eq!(info.nominal_srate(), 250.0);
        assert_eq!(info.channel_format(), ChannelFormat::Float32);
        assert_eq!(info.source_id(), "src1");
        assert!(!info.uid().is_empty());
        assert!(!info.hostname().is_empty());
    }

    #[test]
    fn stream_info_setters() {
        let info = StreamInfo::new("Test", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        info.set_uid("custom-uid");
        assert_eq!(info.uid(), "custom-uid");

        info.set_session_id("session-42");
        assert_eq!(info.session_id(), "session-42");

        info.set_hostname("myhost");
        assert_eq!(info.hostname(), "myhost");

        info.set_v4address("192.168.1.1");
        assert_eq!(info.v4address(), "192.168.1.1");
    }

    #[test]
    fn stream_info_reset_uid() {
        let info = StreamInfo::new("Test", "EEG", 1, 0.0, ChannelFormat::Float32, "");
        let uid1 = info.uid();
        let uid2 = info.reset_uid();
        assert_ne!(uid1, uid2);
        assert_eq!(info.uid(), uid2);
    }

    #[test]
    fn sample_bytes() {
        let info = StreamInfo::new("Test", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert_eq!(info.channel_bytes(), 4);
        assert_eq!(info.sample_bytes(), 32);

        let info2 = StreamInfo::new("Test", "EEG", 8, 250.0, ChannelFormat::Double64, "");
        assert_eq!(info2.sample_bytes(), 64);
    }

    #[test]
    fn shortinfo_xml_generation() {
        let info = StreamInfo::new("MyStream", "EEG", 4, 250.0, ChannelFormat::Float32, "src1");
        info.set_uid("test-uid-123");
        let xml = info.to_shortinfo_message();

        assert!(xml.contains("<name>MyStream</name>"));
        assert!(xml.contains("<type>EEG</type>"));
        assert!(xml.contains("<channel_count>4</channel_count>"));
        assert!(xml.contains("<channel_format>float32</channel_format>"));
        assert!(xml.contains("<source_id>src1</source_id>"));
        assert!(xml.contains("<uid>test-uid-123</uid>"));
        assert!(xml.contains("<desc></desc>"));
    }

    #[test]
    fn fullinfo_xml_with_desc() {
        let info = StreamInfo::new("MyStream", "EEG", 2, 250.0, ChannelFormat::Float32, "");
        let desc = info.desc();
        desc.append_child_value("manufacturer", "TestCorp");

        let xml = info.to_fullinfo_message();
        assert!(xml.contains("<manufacturer>TestCorp</manufacturer>"));
        assert!(xml.contains("<desc>"));
    }

    #[test]
    fn shortinfo_roundtrip() {
        let info = StreamInfo::new(
            "RoundTrip",
            "Markers",
            1,
            0.0,
            ChannelFormat::String,
            "rt_src",
        );
        info.set_uid("fixed-uid");
        info.set_hostname("testhost");

        let xml = info.to_shortinfo_message();
        let parsed = StreamInfo::from_shortinfo_message(&xml).unwrap();

        assert_eq!(parsed.name(), "RoundTrip");
        assert_eq!(parsed.type_(), "Markers");
        assert_eq!(parsed.channel_count(), 1);
        assert_eq!(parsed.nominal_srate(), 0.0);
        assert_eq!(parsed.channel_format(), ChannelFormat::String);
        assert_eq!(parsed.source_id(), "rt_src");
        assert_eq!(parsed.uid(), "fixed-uid");
        assert_eq!(parsed.hostname(), "testhost");
    }

    #[test]
    fn from_shortinfo_invalid_xml() {
        assert!(StreamInfo::from_shortinfo_message("garbage").is_none());
        assert!(StreamInfo::from_shortinfo_message("").is_none());
        assert!(StreamInfo::from_shortinfo_message("<info></info>").is_none());
    }

    #[test]
    fn query_empty_always_matches() {
        let info = StreamInfo::new("Any", "Any", 1, 0.0, ChannelFormat::Float32, "");
        assert!(info.matches_query(""));
    }

    #[test]
    fn query_name_equality() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert!(info.matches_query("name='MyEEG'"));
        assert!(!info.matches_query("name='Other'"));
    }

    #[test]
    fn query_and_or() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "src1");
        assert!(info.matches_query("name='MyEEG' and type='EEG'"));
        assert!(!info.matches_query("name='MyEEG' and type='EMG'"));
        assert!(info.matches_query("name='Other' or type='EEG'"));
        assert!(!info.matches_query("name='Other' or type='EMG'"));
    }

    #[test]
    fn query_numeric_comparisons() {
        let info = StreamInfo::new("Test", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert!(info.matches_query("channel_count>4"));
        assert!(info.matches_query("channel_count>=8"));
        assert!(info.matches_query("channel_count<100"));
        assert!(info.matches_query("channel_count<=8"));
        assert!(!info.matches_query("channel_count>8"));
        assert!(!info.matches_query("channel_count<8"));
    }

    #[test]
    fn query_functions() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert!(info.matches_query("starts-with(name,'My')"));
        assert!(info.matches_query("contains(name,'EEG')"));
        assert!(!info.matches_query("starts-with(name,'X')"));
        assert!(!info.matches_query("contains(name,'XYZ')"));
    }

    #[test]
    fn query_not() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert!(info.matches_query("not(name='Other')"));
        assert!(!info.matches_query("not(name='MyEEG')"));
    }

    #[test]
    fn query_inequality() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "");
        assert!(info.matches_query("name!='Other'"));
        assert!(!info.matches_query("name!='MyEEG'"));
    }

    #[test]
    fn xml_escaping_in_name() {
        let info = StreamInfo::new(
            "Stream<1>&\"test\"",
            "T&ype",
            1,
            0.0,
            ChannelFormat::Float32,
            "",
        );
        let xml = info.to_shortinfo_message();
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&amp;"));

        let parsed = StreamInfo::from_shortinfo_message(&xml).unwrap();
        assert_eq!(parsed.name(), "Stream<1>&\"test\"");
        assert_eq!(parsed.type_(), "T&ype");
    }

    #[test]
    fn clone_shares_state() {
        let info = StreamInfo::new("Test", "EEG", 1, 0.0, ChannelFormat::Float32, "");
        let info2 = info.clone();
        info.set_hostname("changed");
        assert_eq!(info2.hostname(), "changed");
    }
}

// ── postproc tests ───────────────────────────────────────────────────

mod postproc_tests {
    use lsl_core::postproc::TimestampPostProcessor;
    use lsl_core::types::*;

    #[test]
    fn proc_none_passthrough() {
        let mut pp = TimestampPostProcessor::new(PROC_NONE, 250.0, 90.0);
        assert_eq!(pp.process(1.0), 1.0);
        assert_eq!(pp.process(2.0), 2.0);
        assert_eq!(pp.process(3.0), 3.0);
    }

    #[test]
    fn clocksync_adds_offset() {
        let mut pp = TimestampPostProcessor::new(PROC_CLOCKSYNC, 250.0, 90.0);
        pp.set_clock_offset(0.5);
        assert!((pp.process(1.0) - 1.5).abs() < 1e-10);
        assert!((pp.process(2.0) - 2.5).abs() < 1e-10);
    }

    #[test]
    fn clocksync_negative_offset() {
        let mut pp = TimestampPostProcessor::new(PROC_CLOCKSYNC, 250.0, 90.0);
        pp.set_clock_offset(-0.1);
        assert!((pp.process(1.0) - 0.9).abs() < 1e-10);
    }

    #[test]
    fn monotonize_enforces_increasing() {
        let mut pp = TimestampPostProcessor::new(PROC_MONOTONIZE, 0.0, 0.0);
        let t1 = pp.process(1.0);
        let t2 = pp.process(0.5); // goes backward
        let t3 = pp.process(1.0); // still behind
        assert_eq!(t1, 1.0);
        assert!(t2 > t1);
        assert!(t3 > t2);
    }

    #[test]
    fn monotonize_equal_timestamps() {
        let mut pp = TimestampPostProcessor::new(PROC_MONOTONIZE, 0.0, 0.0);
        let t1 = pp.process(5.0);
        let t2 = pp.process(5.0);
        assert!(t2 > t1);
    }

    #[test]
    fn dejitter_smooths_timestamps() {
        let mut pp = TimestampPostProcessor::new(PROC_DEJITTER, 100.0, 90.0);
        // Regular samples at 100Hz = 0.01s interval
        let base = 100.0;
        let mut outputs = Vec::new();
        for i in 0..100 {
            let jittered = base + i as f64 * 0.01 + if i % 2 == 0 { 0.002 } else { -0.002 };
            outputs.push(pp.process(jittered));
        }

        // After smoothing, the jitter should be reduced
        // Calculate jitter of output
        let intervals: Vec<f64> = outputs.windows(2).map(|w| w[1] - w[0]).collect();
        let mean_interval: f64 = intervals.iter().sum::<f64>() / intervals.len() as f64;
        let jitter: f64 = intervals
            .iter()
            .map(|i| (i - mean_interval).powi(2))
            .sum::<f64>()
            / intervals.len() as f64;
        let jitter = jitter.sqrt();

        // The smoothed output should have less jitter than the input (0.004s)
        assert!(
            jitter < 0.003,
            "Dejitter should reduce jitter, got {}",
            jitter
        );
    }

    #[test]
    fn dejitter_irregular_rate_passthrough() {
        // srate=0 means irregular rate, dejitter should still work
        let mut pp = TimestampPostProcessor::new(PROC_DEJITTER, 0.0, 90.0);
        // With srate=0, dejitter is skipped
        assert_eq!(pp.process(1.0), 1.0);
        assert_eq!(pp.process(2.0), 2.0);
    }

    #[test]
    fn all_processors_combined() {
        let mut pp = TimestampPostProcessor::new(PROC_ALL, 250.0, 90.0);
        pp.set_clock_offset(0.1);
        let t1 = pp.process(1.0);
        let t2 = pp.process(1.003); // ~4ms, close to expected 4ms at 250Hz
        let t3 = pp.process(0.5); // backward timestamp
        assert!(t1 > 0.0);
        assert!(t2 > t1); // monotonize
        assert!(t3 > t2); // monotonize
    }

    #[test]
    fn reset_clears_state() {
        let mut pp = TimestampPostProcessor::new(PROC_DEJITTER | PROC_MONOTONIZE, 250.0, 90.0);
        pp.process(100.0);
        pp.process(100.004);
        pp.reset();
        // After reset, should behave like fresh
        let t = pp.process(1.0);
        assert!(t > 0.0 && t < 2.0);
    }
}

// ── signal_quality tests ─────────────────────────────────────────────

mod signal_quality_tests {
    use lsl_core::signal_quality::SignalQuality;

    #[test]
    fn empty_quality() {
        let sq = SignalQuality::new(250.0, 4);
        let snap = sq.snapshot();
        assert_eq!(snap.total_samples, 0);
        assert_eq!(snap.total_dropouts, 0);
        assert_eq!(snap.effective_srate, 0.0);
    }

    #[test]
    fn single_sample() {
        let mut sq = SignalQuality::new(250.0, 2);
        sq.update(1.0, &[1.0, 2.0]);
        let snap = sq.snapshot();
        assert_eq!(snap.total_samples, 1);
    }

    #[test]
    fn effective_sample_rate() {
        let mut sq = SignalQuality::new(100.0, 1);
        // Feed 100 samples at exact 100Hz
        for i in 0..100 {
            sq.update(i as f64 * 0.01, &[0.0]);
        }
        let snap = sq.snapshot();
        assert!(
            (snap.effective_srate - 100.0).abs() < 1.0,
            "Expected ~100Hz, got {}",
            snap.effective_srate
        );
    }

    #[test]
    fn dropout_detection() {
        let mut sq = SignalQuality::new(100.0, 1);
        // Normal samples
        for i in 0..50 {
            sq.update(i as f64 * 0.01, &[0.0]);
        }
        // Gap: skip 10 samples (jump 0.1s)
        for i in 60..100 {
            sq.update(i as f64 * 0.01, &[0.0]);
        }
        let snap = sq.snapshot();
        assert!(
            snap.total_dropouts > 0,
            "Should detect dropouts, got {}",
            snap.total_dropouts
        );
    }

    #[test]
    fn jitter_measurement() {
        let mut sq = SignalQuality::new(100.0, 1);
        // Perfect timing → near-zero jitter
        for i in 0..200 {
            sq.update(i as f64 * 0.01, &[0.0]);
        }
        let snap = sq.snapshot();
        assert!(
            snap.jitter_sec < 0.0001,
            "Perfect timing should have near-zero jitter, got {}",
            snap.jitter_sec
        );
    }

    #[test]
    fn snr_calculation() {
        let mut sq = SignalQuality::new(100.0, 1);
        // Constant signal → infinite SNR
        for i in 0..100 {
            sq.update(i as f64 * 0.01, &[100.0]);
        }
        let snap = sq.snapshot();
        assert!(snap.snr_db.len() == 1, "Should have 1 channel SNR");
        // Constant signal has zero variance → infinite SNR
        assert!(snap.snr_db[0].is_infinite());
    }

    #[test]
    fn reset_clears_all() {
        let mut sq = SignalQuality::new(100.0, 2);
        for i in 0..50 {
            sq.update(i as f64 * 0.01, &[1.0, 2.0]);
        }
        assert!(sq.total_samples > 0);
        sq.reset();
        assert_eq!(sq.total_samples, 0);
        assert_eq!(sq.total_dropouts, 0);
        let snap = sq.snapshot();
        assert_eq!(snap.effective_srate, 0.0);
    }

    #[test]
    fn irregular_rate() {
        let mut sq = SignalQuality::new(0.0, 1); // irregular
        sq.update(1.0, &[0.0]);
        sq.update(1.5, &[0.0]);
        sq.update(3.0, &[0.0]);
        let snap = sq.snapshot();
        assert_eq!(snap.total_samples, 3);
        // No dropouts expected for irregular rate (srate=0)
        assert_eq!(snap.total_dropouts, 0);
    }
}

// ── config tests ─────────────────────────────────────────────────────

mod config_tests {
    use lsl_core::config::CONFIG;

    #[test]
    fn config_has_defaults() {
        assert_eq!(CONFIG.multicast_port, 16571);
        assert_eq!(CONFIG.base_port, 16572);
        assert_eq!(CONFIG.port_range, 32);
        assert!(CONFIG.allow_ipv4);
        assert!(!CONFIG.multicast_addresses.is_empty());
        assert!(CONFIG.smoothing_halftime > 0.0);
    }

    #[test]
    fn config_has_time_correction_params() {
        assert!(CONFIG.time_probe_count > 0);
        assert!(CONFIG.time_probe_interval > 0.0);
        assert!(CONFIG.time_probe_max_rtt > 0.0);
        assert!(CONFIG.time_update_minprobes > 0);
        assert!(CONFIG.time_update_interval > 0.0);
    }
}
