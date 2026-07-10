use super::*;
use crate::memory::MemoryCategory;

fn make_test_memory(content: &str) -> MemoryEntry {
    MemoryEntry::new(MemoryCategory::Fact, content)
}

#[test]
fn test_new_graph() {
    let graph = MemoryGraph::new();
    assert_eq!(graph.graph_version, GRAPH_VERSION);
    assert!(graph.memories.is_empty());
    assert!(graph.tags.is_empty());
}

#[test]
fn test_add_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test content");
    let id = graph.add_memory(entry);

    assert!(graph.memories.contains_key(&id));
    assert_eq!(graph.get_memory(&id).unwrap().content, "Test content");
}

#[test]
fn test_add_memory_with_tags() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Uses tokio").with_tags(vec!["rust".into(), "async".into()]);
    let id = graph.add_memory(entry);

    // Tags should be created
    assert!(graph.tags.contains_key("tag:rust"));
    assert!(graph.tags.contains_key("tag:async"));

    // Edges should exist
    let edges = graph.get_edges(&id);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().any(|e| e.target == "tag:rust"));
    assert!(edges.iter().any(|e| e.target == "tag:async"));
}

#[test]
fn test_tag_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test");
    let id = graph.add_memory(entry);

    graph.tag_memory(&id, "newtag");

    assert!(graph.tags.contains_key("tag:newtag"));
    assert_eq!(graph.tags.get("tag:newtag").unwrap().count, 1);

    let memory = graph.get_memory(&id).unwrap();
    assert!(memory.tags.contains(&"newtag".to_string()));
}

#[test]
fn test_untag_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test").with_tags(vec!["removeme".into()]);
    let id = graph.add_memory(entry);

    graph.untag_memory(&id, "removeme");

    let memory = graph.get_memory(&id).unwrap();
    assert!(!memory.tags.contains(&"removeme".to_string()));
    assert_eq!(graph.tags.get("tag:removeme").unwrap().count, 0);
}

#[test]
fn test_get_memories_by_tag() {
    let mut graph = MemoryGraph::new();

    let entry1 = make_test_memory("Memory 1").with_tags(vec!["shared".into()]);
    let entry2 = make_test_memory("Memory 2").with_tags(vec!["shared".into()]);
    let entry3 = make_test_memory("Memory 3").with_tags(vec!["other".into()]);

    graph.add_memory(entry1);
    graph.add_memory(entry2);
    graph.add_memory(entry3);

    let shared = graph.get_memories_by_tag("shared");
    assert_eq!(shared.len(), 2);

    let other = graph.get_memories_by_tag("other");
    assert_eq!(other.len(), 1);
}

#[test]
fn test_link_memories() {
    let mut graph = MemoryGraph::new();
    let id1 = graph.add_memory(make_test_memory("Memory A"));
    let id2 = graph.add_memory(make_test_memory("Memory B"));

    graph.link_memories(&id1, &id2, 0.8);

    let edges = graph.get_edges(&id1);
    assert!(
        edges.iter().any(|e| e.target == id2
            && e.kind == EdgeKind::SimilarTo
            && (e.meta.weight - 0.8).abs() < 1e-6)
    );
}

#[test]
fn test_supersede() {
    let mut graph = MemoryGraph::new();
    let old_id = graph.add_memory(make_test_memory("Old info"));
    let new_id = graph.add_memory(make_test_memory("New info"));

    graph.supersede(&new_id, &old_id);

    let old = graph.get_memory(&old_id).unwrap();
    assert!(!old.active);
    assert_eq!(old.superseded_by, Some(new_id.clone()));

    let edges = graph.get_edges(&new_id);
    assert!(
        edges
            .iter()
            .any(|e| e.target == old_id && matches!(e.kind, EdgeKind::Supersedes))
    );
}

#[test]
fn test_remove_memory() {
    let mut graph = MemoryGraph::new();
    let entry = make_test_memory("Test").with_tags(vec!["tag1".into()]);
    let id = graph.add_memory(entry);

    assert!(graph.memories.contains_key(&id));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 1);

    graph.remove_memory(&id);

    assert!(!graph.memories.contains_key(&id));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 0);
    assert!(graph.get_edges(&id).is_empty());
}

#[test]
fn test_node_and_edge_counts() {
    let mut graph = MemoryGraph::new();

    let entry1 = make_test_memory("M1").with_tags(vec!["t1".into()]);
    let entry2 = make_test_memory("M2").with_tags(vec!["t1".into(), "t2".into()]);

    graph.add_memory(entry1);
    graph.add_memory(entry2);

    // 2 memories + 2 tags = 4 nodes
    assert_eq!(graph.node_count(), 4);
    // M1->t1, M2->t1, M2->t2 = 3 edges
    assert_eq!(graph.edge_count(), 3);
}

#[test]
fn test_cascade_retrieval_through_tags() {
    let mut graph = MemoryGraph::new();

    // Create: A --HasTag--> tag:rust <--HasTag-- B
    //         A --HasTag--> tag:async <--HasTag-- C
    let id_a = graph
        .add_memory(make_test_memory("Memory A").with_tags(vec!["rust".into(), "async".into()]));
    let id_b = graph.add_memory(make_test_memory("Memory B").with_tags(vec!["rust".into()]));
    let id_c = graph.add_memory(make_test_memory("Memory C").with_tags(vec!["async".into()]));

    // Start from A with score 1.0
    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);

    // Should find A (seed), B (via rust tag), C (via async tag)
    assert!(results.iter().any(|(id, _)| id == &id_a));
    assert!(results.iter().any(|(id, _)| id == &id_b));
    assert!(results.iter().any(|(id, _)| id == &id_c));

    // A should have highest score (seed)
    let a_score = results
        .iter()
        .find(|(id, _)| id == &id_a)
        .map(|(_, s)| *s)
        .unwrap();
    let b_score = results
        .iter()
        .find(|(id, _)| id == &id_b)
        .map(|(_, s)| *s)
        .unwrap();
    assert!(a_score > b_score);
}

#[test]
fn test_cascade_retrieval_respects_result_limit_and_order() {
    let mut graph = MemoryGraph::new();

    let id_a = graph.add_memory(make_test_memory("Memory A"));
    let id_b = graph.add_memory(make_test_memory("Memory B"));
    let id_c = graph.add_memory(make_test_memory("Memory C"));
    let id_d = graph.add_memory(make_test_memory("Memory D"));

    graph.link_memories(&id_a, &id_b, 0.9);
    graph.link_memories(&id_a, &id_c, 0.8);
    graph.link_memories(&id_a, &id_d, 0.7);

    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 1, 3);

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, id_a);
    assert_eq!(results[1].0, id_b);
    assert_eq!(results[2].0, id_c);
    assert!(results[0].1 > results[1].1);
    assert!(results[1].1 > results[2].1);
}

#[test]
fn test_cascade_retrieval_respects_depth() {
    let mut graph = MemoryGraph::new();

    // Create chain: A --tag:t1--> B --tag:t2--> C --tag:t3--> D
    let id_a = graph.add_memory(make_test_memory("A").with_tags(vec!["t1".into()]));
    let id_b = graph.add_memory(make_test_memory("B").with_tags(vec!["t1".into(), "t2".into()]));
    let id_c = graph.add_memory(make_test_memory("C").with_tags(vec!["t2".into(), "t3".into()]));
    let _id_d = graph.add_memory(make_test_memory("D").with_tags(vec!["t3".into()]));

    // Depth 1: should find A, B (via t1)
    let results_d1 = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 1, 10);
    assert!(results_d1.iter().any(|(id, _)| id == &id_a));
    assert!(results_d1.iter().any(|(id, _)| id == &id_b));

    // Depth 2: should find A, B, C (via t1->t2)
    let results_d2 = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);
    assert!(results_d2.iter().any(|(id, _)| id == &id_c));
}

#[test]
fn test_cascade_retrieval_via_relates_to() {
    let mut graph = MemoryGraph::new();

    let id_a = graph.add_memory(make_test_memory("Memory A"));
    let id_b = graph.add_memory(make_test_memory("Memory B"));
    let id_c = graph.add_memory(make_test_memory("Memory C"));

    // A --RelatesTo(0.8)--> B --RelatesTo(0.7)--> C
    graph.link_memories(&id_a, &id_b, 0.8);
    graph.link_memories(&id_b, &id_c, 0.7);

    let results = graph.cascade_retrieve(std::slice::from_ref(&id_a), &[1.0], 2, 10);

    // Should find all three
    assert!(results.iter().any(|(id, _)| id == &id_a));
    assert!(results.iter().any(|(id, _)| id == &id_b));
    assert!(results.iter().any(|(id, _)| id == &id_c));
}

#[test]
fn test_migration_from_legacy() {
    // Create a legacy MemoryStore
    let mut old_store = MemoryStore::new();
    old_store.add(make_test_memory("Memory 1").with_tags(vec!["tag1".into(), "tag2".into()]));
    old_store.add(make_test_memory("Memory 2").with_tags(vec!["tag1".into()]));

    // Migrate
    let graph = MemoryGraph::from_legacy_store(old_store);

    // Check version
    assert_eq!(graph.graph_version, GRAPH_VERSION);

    // Check memories migrated
    assert_eq!(graph.memories.len(), 2);

    // Check tags created
    assert!(graph.tags.contains_key("tag:tag1"));
    assert!(graph.tags.contains_key("tag:tag2"));
    assert_eq!(graph.tags.get("tag:tag1").unwrap().count, 2);
    assert_eq!(graph.tags.get("tag:tag2").unwrap().count, 1);

    // Check edges exist
    let edges_total: usize = graph.edges.values().map(|v| v.len()).sum();
    assert_eq!(edges_total, 3); // 2 edges for M1, 1 for M2
}

#[test]
fn test_graph_serialization_roundtrip() {
    let mut graph = MemoryGraph::new();

    // Add a memory with tags
    let entry = make_test_memory("Test memory").with_tags(vec!["rust".into()]);
    let id = graph.add_memory(entry);

    // Manually add a tag edge to verify serialization
    graph.tag_memory(&id, "extra");

    // Serialize
    let json = serde_json::to_string_pretty(&graph).expect("serialize");
    eprintln!("Serialized graph:\n{}", json);

    // Check edges appear in JSON
    assert!(json.contains("\"edges\""), "JSON should contain edges key");
    assert!(
        json.contains("tag:rust") || json.contains("tag:extra"),
        "JSON should contain tag references"
    );

    // Deserialize
    let parsed: MemoryGraph = serde_json::from_str(&json).expect("deserialize");

    // Verify
    assert_eq!(parsed.memories.len(), 1);
    assert_eq!(parsed.tags.len(), 2); // rust and extra
    assert_eq!(
        parsed.edge_count(),
        graph.edge_count(),
        "Edge count should match after roundtrip"
    );
}

/// Read the weight of the SimilarTo edge a->b, if any.
fn relates_weight(graph: &MemoryGraph, a: &str, b: &str) -> Option<f32> {
    graph.get_edges(a).iter().find_map(|e| {
        if e.target == b && e.kind == EdgeKind::SimilarTo {
            return Some(e.meta.weight);
        }
        None
    })
}

#[test]
fn reinforce_link_is_symmetric_and_saturates() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("alpha"));
    let b = graph.add_memory(make_test_memory("beta"));

    graph.reinforce_link(&a, &b, 0.25);
    assert!((relates_weight(&graph, &a, &b).unwrap() - 0.25).abs() < 1e-6);
    // Symmetric: reverse edge exists too.
    assert!((relates_weight(&graph, &b, &a).unwrap() - 0.25).abs() < 1e-6);

    // Repeated co-activation strengthens, capped at 1.0.
    for _ in 0..10 {
        graph.reinforce_link(&a, &b, 0.25);
    }
    assert!((relates_weight(&graph, &a, &b).unwrap() - 1.0).abs() < 1e-6);

    // Unknown ids are a no-op (don't panic / create edges).
    graph.reinforce_link(&a, "mem_missing", 0.5);
    assert!(relates_weight(&graph, &a, "mem_missing").is_none());
}

#[test]
fn bootstrap_cooccurrence_links_connects_shared_tags() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(
        make_test_memory("sidecar tokenizes input").with_tags(vec!["sidecar".into(), "tokens".into()]),
    );
    let b = graph.add_memory(
        make_test_memory("sidecar detokenizes output")
            .with_tags(vec!["sidecar".into(), "tokens".into()]),
    );
    let c = graph
        .add_memory(make_test_memory("gardening tomatoes").with_tags(vec!["garden".into()]));

    let linked = graph.bootstrap_cooccurrence_links(0.34);
    assert!(linked >= 1, "shared-tag pair should link");

    // a<->b share both tags (Jaccard 1.0) → linked symmetrically.
    assert!(relates_weight(&graph, &a, &b).is_some());
    assert!(relates_weight(&graph, &b, &a).is_some());
    // c shares nothing → not linked to a.
    assert!(relates_weight(&graph, &a, &c).is_none());

    // Idempotent-ish: re-running never lowers the weight.
    let w1 = relates_weight(&graph, &a, &b).unwrap();
    graph.bootstrap_cooccurrence_links(0.34);
    let w2 = relates_weight(&graph, &a, &b).unwrap();
    assert!(w2 >= w1 - 1e-6);
}

#[test]
fn decay_relates_to_weakens_and_prunes() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("alpha"));
    let b = graph.add_memory(make_test_memory("beta"));
    let c = graph.add_memory(make_test_memory("gamma"));

    graph.reinforce_link(&a, &b, 0.9); // strong
    graph.reinforce_link(&a, &c, 0.18); // weak, will fall below floor

    let (weakened, pruned) = graph.decay_relates_to(0.5, 0.15);
    assert!(weakened >= 2);
    // a->c: 0.18 * 0.5 = 0.09 < 0.15 → pruned (both directions).
    assert_eq!(pruned, 2, "weak link pruned in both directions");
    assert!(relates_weight(&graph, &a, &c).is_none());
    assert!(relates_weight(&graph, &c, &a).is_none());
    // a->b: 0.9 * 0.5 = 0.45 survives.
    assert!((relates_weight(&graph, &a, &b).unwrap() - 0.45).abs() < 1e-6);
}

#[test]
fn edge_type_counts_and_hubs() {
    let mut graph = MemoryGraph::new();
    let hub = graph.add_memory(make_test_memory("hub").with_tags(vec!["x".into(), "y".into()]));
    let b = graph.add_memory(make_test_memory("b"));
    let c = graph.add_memory(make_test_memory("c"));
    graph.reinforce_link(&hub, &b, 0.5);
    graph.reinforce_link(&hub, &c, 0.5);

    let counts = graph.edge_type_counts();
    assert_eq!(counts.get("has_tag").copied().unwrap_or(0), 2, "two has_tag edges");
    assert_eq!(
        counts.get("similar_to").copied().unwrap_or(0),
        4,
        "two symmetric similar_to = 4 directed"
    );

    let hubs = graph.top_hubs(1);
    assert_eq!(hubs[0].0, hub, "hub should be the most-connected memory");
}

// ==================== v0.11 semantic memory ====================

#[test]
fn legacy_relates_to_edge_deserializes_as_similar_to() {
    // Old on-disk edge shape: {"target":"x","kind":"relates_to","weight":0.8}
    let json = r#"{"target":"mem_x","kind":"relates_to","weight":0.8}"#;
    let edge: Edge = serde_json::from_str(json).expect("deserialize legacy edge");
    assert_eq!(edge.kind, EdgeKind::SimilarTo);
    assert!((edge.meta.weight - 0.8).abs() < 1e-6);
    // Round-trips to the new label.
    let out = serde_json::to_string(&edge).unwrap();
    assert!(out.contains("\"similar_to\""), "serialized: {out}");
}

#[test]
fn typed_edges_carry_evidence_and_confidence() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("fact a"));
    let b = graph.add_memory(make_test_memory("supporting b"));
    graph.add_typed_edge(
        &b,
        &a,
        EdgeKind::Supports,
        0.9,
        EdgeSource::Llm,
        Some(EvidenceRef::observation("b substantiates a")),
    );
    let e = graph
        .get_edges(&b)
        .iter()
        .find(|e| e.target == a && e.kind == EdgeKind::Supports)
        .expect("supports edge");
    assert_eq!(e.meta.source, EdgeSource::Llm);
    assert_eq!(e.meta.evidence_count, 1);
    assert!(e.meta.confidence > 0.0, "confidence must be evidence-backed");
    assert_eq!(e.meta.evidence.len(), 1);
    // Supports is directional: no reverse edge.
    assert!(graph.get_edges(&a).iter().all(|e| e.target != b));
}

#[test]
fn hebbian_only_reinforces_associative_kinds() {
    assert!(EdgeKind::SimilarTo.is_reinforceable());
    assert!(EdgeKind::Supports.is_reinforceable());
    assert!(!EdgeKind::Causes.is_reinforceable());
    assert!(!EdgeKind::Before.is_reinforceable());
    assert!(!EdgeKind::PartOf.is_reinforceable());

    // decay only touches reinforceable edges.
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("a"));
    let b = graph.add_memory(make_test_memory("b"));
    graph.add_typed_edge(&a, &b, EdgeKind::Causes, 0.9, EdgeSource::Manual, None);
    graph.reinforce_link(&a, &b, 0.5); // similar_to
    let before_causes = graph
        .get_edges(&a)
        .iter()
        .find(|e| e.kind == EdgeKind::Causes)
        .map(|e| e.meta.weight)
        .unwrap();
    graph.decay_relates_to(0.5, 0.0);
    let after_causes = graph
        .get_edges(&a)
        .iter()
        .find(|e| e.kind == EdgeKind::Causes)
        .map(|e| e.meta.weight)
        .unwrap();
    assert!((before_causes - after_causes).abs() < 1e-6, "Causes must not decay");
}

#[test]
fn importance_ranks_connected_recent_high_trust_above_isolated() {
    let mut graph = MemoryGraph::new();

    let mut hot = make_test_memory("central, fresh, trusted");
    hot.trust = crate::memory::TrustLevel::High;
    hot.access_count = 20;
    hot.confidence = 0.95;
    let hot = graph.add_memory(hot);

    let mut cold = make_test_memory("isolated, stale, low trust");
    cold.trust = crate::memory::TrustLevel::Low;
    cold.access_count = 0;
    cold.confidence = 0.2;
    cold.updated_at = chrono::Utc::now() - chrono::Duration::days(120);
    let cold = graph.add_memory(cold);

    // Give hot some neighbours (centrality + connectivity).
    for i in 0..4 {
        let n = graph.add_memory(make_test_memory(&format!("neighbour {i}")));
        graph.reinforce_link(&hot, &n, 0.8);
    }

    assert!(
        graph.importance(&hot) > graph.importance(&cold),
        "connected recent trusted memory should outrank isolated stale one"
    );
    let ranking = graph.importance_ranking(1);
    assert_eq!(ranking[0].0, hot);
}

#[test]
fn detect_communities_groups_two_clusters() {
    let mut graph = MemoryGraph::new();
    // Cluster 1: rust-tagged, densely linked.
    let mut c1 = Vec::new();
    for i in 0..3 {
        let id = graph.add_memory(
            make_test_memory(&format!("rust concept {i}")).with_tags(vec!["rust".into()]),
        );
        c1.push(id);
    }
    // Cluster 2: cooking-tagged, densely linked.
    let mut c2 = Vec::new();
    for i in 0..3 {
        let id = graph.add_memory(
            make_test_memory(&format!("cooking concept {i}")).with_tags(vec!["cooking".into()]),
        );
        c2.push(id);
    }
    for i in 0..c1.len() {
        for j in (i + 1)..c1.len() {
            graph.reinforce_link(&c1[i], &c1[j], 0.9);
        }
    }
    for i in 0..c2.len() {
        for j in (i + 1)..c2.len() {
            graph.reinforce_link(&c2[i], &c2[j], 0.9);
        }
    }

    let n = graph.detect_communities(3, 8);
    assert_eq!(n, 2, "two dense components → two communities");
    // Each member has an InCluster edge.
    let in_cluster = graph.count_edges_of_kind(EdgeKind::InCluster);
    assert_eq!(in_cluster, 6, "all 6 memories assigned to a community");
    // Clusters named after dominant tag.
    let names: std::collections::HashSet<String> =
        graph.clusters.values().filter_map(|c| c.name.clone()).collect();
    assert!(names.contains("rust") && names.contains("cooking"), "names: {names:?}");

    // Re-running is idempotent (stable ids, still 2).
    assert_eq!(graph.detect_communities(3, 8), 2);
    assert_eq!(graph.count_edges_of_kind(EdgeKind::InCluster), 6);
}

#[test]
fn episodic_promotes_to_semantic_after_repeats() {
    let mut graph = MemoryGraph::new();
    let id = graph.add_memory(make_test_memory("Rust ownership prevents double free"));

    let mut promoted_at = None;
    for i in 1..=MemoryGraph::SEMANTIC_STRENGTH {
        let promoted = graph.record_fact_observation(&id, EvidenceRef::observation("seen again"));
        if promoted {
            promoted_at = Some(i);
        }
    }
    assert!(promoted_at.is_some(), "should promote once strength threshold met");
    let m = graph.get_memory(&id).unwrap();
    assert!(m.tags.iter().any(|t| t == "semantic"), "tagged semantic");
    assert!(m.confidence > 0.8, "confidence accrues with evidence: {}", m.confidence);
    assert!(!m.evidence.is_empty(), "evidence recorded");
    // No double promotion.
    assert!(!graph.record_fact_observation(&id, EvidenceRef::observation("again")));
}

#[test]
fn shortest_semantic_path_and_contradictions() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("A"));
    let b = graph.add_memory(make_test_memory("B"));
    let c = graph.add_memory(make_test_memory("C"));
    graph.link_memories(&a, &b, 0.9);
    graph.link_memories(&b, &c, 0.9);

    let path = graph.shortest_semantic_path(&a, &c, 5).expect("path a→c");
    let ids: Vec<&str> = path.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(ids, vec![a.as_str(), b.as_str(), c.as_str()]);

    graph.mark_contradiction(&a, &c);
    assert_eq!(graph.contradictions_of(&a), vec![c.clone()]);
    let (x, y, _) = graph.strongest_contradiction().expect("a contradiction");
    assert!((x == a && y == c) || (x == c && y == a));
}

#[test]
fn sleep_cycle_produces_report() {
    let mut graph = MemoryGraph::new();
    for i in 0..4 {
        graph.add_memory(
            make_test_memory(&format!("kernel note {i}"))
                .with_tags(vec!["kernel".into(), "os".into()]),
        );
    }
    let report = graph.run_sleep_cycle(SleepConfig::default());
    assert!(report.linked > 0, "shared tags should form associations");
    assert!(report.confidence_decayed >= 4, "confidence decay touches all memories");
    // With 4 densely tag-linked memories, one community should form.
    assert!(report.communities >= 1, "a community should emerge");
}

// ==================== v0.12 semantic-memory completion ====================

fn seeded_community_graph() -> (MemoryGraph, Vec<String>) {
    let mut graph = MemoryGraph::new();
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = graph.add_memory(
            make_test_memory(&format!("The DB connection pool caps at 20, note {i}"))
                .with_tags(vec!["database".into(), "config".into()]),
        );
        ids.push(id);
    }
    // Dense associations so label propagation groups them.
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            graph.reinforce_link(&ids[i], &ids[j], 0.9);
        }
    }
    graph.detect_communities(3, 8);
    (graph, ids)
}

#[test]
fn consolidation_links_provenance_and_is_idempotent() {
    let (mut graph, ids) = seeded_community_graph();

    let groups = graph.consolidation_candidates(2);
    assert_eq!(groups.len(), 1, "one ripe community");
    let group = &groups[0];
    assert_eq!(group.len(), 3);

    let sem = graph
        .apply_consolidation(group, "The database connection pool is capped at 20.", "database")
        .expect("semantic memory created");

    // Semantic node exists, tagged, and DerivedFrom every episode.
    let m = graph.get_memory(&sem).unwrap();
    assert!(m.tags.iter().any(|t| t == "semantic"));
    assert!(m.tags.iter().any(|t| t == "consolidated"));
    let sources = graph.derived_sources(&sem);
    assert_eq!(sources.len(), 3, "linked to all three episodes");
    for id in &ids {
        assert!(sources.contains(id));
        // Originals are preserved & still active (never deleted).
        assert!(graph.get_memory(id).unwrap().active);
    }
    assert_eq!(graph.metadata.consolidations.len(), 1, "history recorded");

    // Idempotent: same group → same id, no duplicate history explosion.
    let sem2 = graph
        .apply_consolidation(group, "The database connection pool caps at 20.", "database")
        .unwrap();
    assert_eq!(sem, sem2, "stable id from member set");
    assert_eq!(graph.derived_sources(&sem).len(), 3, "no duplicate edges");

    // No duplicate semantic memories.
    assert!(graph.validate().iter().all(|i| i.category() != "duplicate_semantic_memory"));
}

#[test]
fn contradiction_candidates_and_apply_is_graph_only() {
    let mut graph = MemoryGraph::new();
    let mut a = make_test_memory("The service listens on port 8080.");
    a.tags.push("semantic".into());
    a.confidence = 0.9;
    let mut b = make_test_memory("The service listens on port 9090.");
    b.tags.push("semantic".into());
    b.confidence = 0.9;
    let a = graph.add_memory(a);
    let b = graph.add_memory(b);

    let pairs = graph.contradiction_candidates(0.6, 10);
    assert_eq!(pairs.len(), 1, "one high-confidence semantic pair");

    let (conf_a_before, conf_b_before) = (
        graph.get_memory(&a).unwrap().confidence,
        graph.get_memory(&b).unwrap().confidence,
    );
    graph.apply_contradiction(&a, &b, "ports 8080 and 9090 cannot both be the listen port");

    // Symmetric Contradicts edge with reasoning as evidence.
    assert!(graph.contradictions_of(&a).contains(&b));
    assert!(graph.contradictions_of(&b).contains(&a));
    let edge = graph.get_edges(&a).into_iter().find(|e| e.kind == EdgeKind::Contradicts).unwrap();
    assert!(edge.meta.evidence.iter().any(|ev| ev.note.is_some()));
    assert_eq!(edge.meta.source, EdgeSource::Llm);

    // Never modifies memory confidence.
    assert_eq!(graph.get_memory(&a).unwrap().confidence, conf_a_before);
    assert_eq!(graph.get_memory(&b).unwrap().confidence, conf_b_before);

    // Re-review skips the already-recorded pair.
    assert!(graph.contradiction_candidates(0.6, 10).is_empty());
}

#[test]
fn build_concept_text_includes_neighbors_and_community() {
    let (graph, ids) = seeded_community_graph();
    let text = graph.build_concept_text(&ids[0]).expect("concept text");
    assert!(text.contains("connection pool"), "includes own content");
    assert!(text.contains("similar_to"), "includes typed relationships");
    assert!(text.contains("[concept:"), "includes community label");
    assert!(text.contains("[tags:"), "includes tags");
}

#[test]
fn refresh_concept_embeddings_uses_stub_embedder() {
    let (mut graph, ids) = seeded_community_graph();
    let n = graph.refresh_concept_embeddings(|t| Some(vec![t.len() as f32, 1.0, 2.0]));
    assert_eq!(n, ids.len(), "every active memory gets a concept embedding");
    assert!(graph.get_memory(&ids[0]).unwrap().concept_embedding.is_some());
    assert!(graph.metadata.last_concept_embed.is_some());
}

#[test]
fn validate_flags_and_repair_fixes_dangling_edges() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("anchor"));
    // Inject a dangling edge to a non-existent target.
    graph.add_edge_internal(&a, "mem-does-not-exist", EdgeKind::SimilarTo);

    let issues = graph.validate();
    assert!(
        issues.iter().any(|i| i.category() == "dangling_edge_target"),
        "dangling target flagged: {issues:?}"
    );

    let fixed = graph.repair();
    assert!(fixed >= 1, "repair dropped the dangling edge");
    assert!(
        graph.validate().iter().all(|i| i.category() != "dangling_edge_target"),
        "no dangling edges after repair"
    );
}

#[test]
fn validate_detects_cyclic_supersedes() {
    let mut graph = MemoryGraph::new();
    let a = graph.add_memory(make_test_memory("v1"));
    let b = graph.add_memory(make_test_memory("v2"));
    graph.add_edge_internal(&a, &b, EdgeKind::Supersedes);
    graph.add_edge_internal(&b, &a, EdgeKind::Supersedes);
    assert!(
        graph.validate().iter().any(|i| i.category() == "cyclic_supersedes"),
        "cycle detected"
    );
}
