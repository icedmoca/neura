#[test]
fn test_last_render_state_equality_requires_all_fields() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let s1 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };
    let s2 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };
    let s3 = LastRenderState {
        area,
        crop_top: true,
        resize_mode: ResizeMode::Fit,
    };
    let s4 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Crop,
    };
    let s5 = LastRenderState {
        area: Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        },
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };

    assert_eq!(s1, s2, "identical states should be equal");
    assert_ne!(s1, s3, "different crop_top should not be equal");
    assert_ne!(s1, s4, "different resize_mode should not be equal");
    assert_ne!(s1, s5, "different area should not be equal");
}

#[test]
fn test_debug_stats_aggregate_across_renders() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let initial_requests = MERMAID_DEBUG.lock().unwrap().stats.total_requests;
    let content = "flowchart LR\n    X[Start] --> Y[End]";
    let _ = render_mermaid_untracked(content, None);
    let after_requests = MERMAID_DEBUG.lock().unwrap().stats.total_requests;

    assert!(
        after_requests > initial_requests,
        "total_requests should increment on each render call"
    );

    let stats = MERMAID_DEBUG.lock().unwrap().stats.clone();
    let total_cache = stats.cache_hits + stats.cache_misses;
    assert!(
        total_cache >= after_requests - initial_requests,
        "cache_hits + cache_misses should account for all render calls, \
             got hits={} misses={} requests_delta={}",
        stats.cache_hits,
        stats.cache_misses,
        after_requests - initial_requests
    );
}

#[test]
fn test_kitty_viewport_state_reuses_transmit_for_scroll_only_updates() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }

    let hash = 0x1234_5678_9abc_def0;
    let path = PathBuf::from("/tmp/test-kitty-scroll.png");
    let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        640,
        480,
        image::Rgba([20, 40, 60, 255]),
    ));

    let (unique_id, full_cols, full_rows) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16))
            .expect("kitty viewport state");
    assert!(full_cols > 0 && full_rows > 0);

    let rebuilds_after_first = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;
    assert_eq!(rebuilds_after_first, 1);

    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    assert!(render_kitty_virtual_viewport(
        hash,
        Rect::new(0, 0, 20, 8),
        &mut buf,
        0,
        0,
        20,
        8
    ));
    let first_symbol = buf[(0, 0)].symbol().to_string();
    assert!(
        first_symbol.contains("_Gq=2"),
        "first render should transmit image data"
    );

    let (same_id, _, _) = ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16))
        .expect("kitty viewport state reused");
    assert_eq!(same_id, unique_id);
    let rebuilds_after_second = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;
    assert_eq!(rebuilds_after_second, rebuilds_after_first);

    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    assert!(render_kitty_virtual_viewport(
        hash,
        Rect::new(0, 0, 20, 8),
        &mut buf,
        3,
        2,
        20,
        8
    ));
    let second_symbol = buf[(0, 0)].symbol().to_string();
    assert!(
        !second_symbol.contains("_Gq=2"),
        "scroll-only render should reuse prior transmit"
    );
}

#[test]
fn test_kitty_viewport_state_rebuilds_when_font_size_changes() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }

    let hash = 0x00fa_ce00_dead_beef;
    let path = PathBuf::from("/tmp/test-kitty-font-size.png");
    let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        640,
        480,
        image::Rgba([80, 140, 220, 255]),
    ));

    let (id_small, cols_small, rows_small) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16)).expect("font 8x16");
    let rebuilds_small = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    let (id_large, cols_large, rows_large) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (16, 32)).expect("font 16x32");
    let rebuilds_large = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    assert_eq!(
        id_small, id_large,
        "font-size changes should reuse kitty image id"
    );
    assert!(
        cols_large < cols_small,
        "larger font should reduce column span"
    );
    assert!(
        rows_large < rows_small,
        "larger font should reduce row span"
    );
    assert_eq!(rebuilds_large, rebuilds_small + 1);
}

#[test]
fn test_kitty_viewport_state_rebuilds_when_zoom_changes() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }

    let hash = 0x0bad_f00d_dead_beef;
    let path = PathBuf::from("/tmp/test-kitty-zoom.png");
    let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        320,
        200,
        image::Rgba([200, 120, 80, 255]),
    ));

    let (id_100, cols_100, rows_100) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16)).expect("zoom 100");
    let rebuilds_100 = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    let (id_150, cols_150, rows_150) =
        ensure_kitty_viewport_state(hash, &path, &source, 150, (8, 16)).expect("zoom 150");
    let rebuilds_150 = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    assert_eq!(id_100, id_150, "zoom changes should reuse kitty image id");
    assert!(cols_150 >= cols_100);
    assert!(rows_150 >= rows_100);
    assert_eq!(rebuilds_150, rebuilds_100 + 1);
}
