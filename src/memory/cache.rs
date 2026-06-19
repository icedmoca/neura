use crate::memory_graph::MemoryGraph;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

// === Graph Cache ===

struct GraphCacheEntry {
    graph: MemoryGraph,
    modified: Option<SystemTime>,
}

struct GraphCache {
    entries: HashMap<PathBuf, GraphCacheEntry>,
}

impl GraphCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

static GRAPH_CACHE: OnceLock<Mutex<GraphCache>> = OnceLock::new();

fn graph_cache() -> &'static Mutex<GraphCache> {
    GRAPH_CACHE.get_or_init(|| Mutex::new(GraphCache::new()))
}

fn graph_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

pub(super) fn cached_graph(path: &PathBuf) -> Option<MemoryGraph> {
    let modified = graph_mtime(path);
    let cache = graph_cache().lock().ok()?;
    let entry = cache.entries.get(path)?;
    if entry.modified == modified {
        Some(entry.graph.clone())
    } else {
        None
    }
}

pub(super) fn cache_graph(path: PathBuf, graph: &MemoryGraph) {
    let modified = graph_mtime(&path);
    if let Ok(mut cache) = graph_cache().lock() {
        cache.entries.insert(
            path,
            GraphCacheEntry {
                graph: graph.clone(),
                modified,
            },
        );
    }
}

// === Search Explanation Cache ===

#[derive(Clone)]
pub(super) struct SearchCacheEntry<T: Clone> {
    pub modified: Option<SystemTime>,
    pub results: Vec<T>,
}

static SEARCH_CACHE: OnceLock<
    Mutex<
        HashMap<(PathBuf, String, usize), SearchCacheEntry<crate::memory::MemorySearchExplanation>>,
    >,
> = OnceLock::new();

fn search_cache() -> &'static Mutex<
    HashMap<(PathBuf, String, usize), SearchCacheEntry<crate::memory::MemorySearchExplanation>>,
> {
    SEARCH_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn cached_search(
    path: &PathBuf,
    query: &str,
    limit: usize,
) -> Option<Vec<crate::memory::MemorySearchExplanation>> {
    let modified = graph_mtime(path);
    let cache = search_cache().lock().ok()?;
    let entry = cache.get(&(path.clone(), query.to_string(), limit))?;
    if entry.modified == modified {
        Some(entry.results.clone())
    } else {
        None
    }
}

pub(super) fn cache_search(
    path: PathBuf,
    query: String,
    limit: usize,
    results: &[crate::memory::MemorySearchExplanation],
) {
    let modified = graph_mtime(&path);
    if let Ok(mut cache) = search_cache().lock() {
        cache.insert(
            (path, query, limit),
            SearchCacheEntry {
                modified,
                results: results.to_vec(),
            },
        );
    }
}

pub(super) fn clear_search_cache() {
    if let Some(cache) = SEARCH_CACHE.get() {
        if let Ok(mut cache) = cache.lock() {
            cache.clear();
        }
    }
}
