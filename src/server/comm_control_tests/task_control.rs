#[tokio::test]
async fn task_control_wake_returns_structured_response_with_plan_summary() {
    let (_env, _runtime) = RuntimeEnvGuard::new();
    let swarm_id = "swarm-task-control";
    let requester = "coord";
    let worker = "worker";
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let worker_agent = test_agent().await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        worker.to_string(),
        worker_agent,
    )])));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (requester.to_string(), {
            let mut member = member(requester, swarm_id, "ready");
            member.role = "coordinator".to_string();
            member
        }),
        (worker.to_string(), member(worker, swarm_id, "ready")),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([requester.to_string(), worker.to_string()]),
    )])));
    let mut assigned = plan_item("active-task", "queued", "high", &[]);
    assigned.assigned_to = Some(worker.to_string());
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        VersionedPlan {
            items: vec![assigned, plan_item("next", "queued", "high", &[])],
            version: 1,
            participants: HashSet::from([requester.to_string(), worker.to_string()]),
            task_progress: HashMap::new(),
        },
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        requester.to_string(),
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(1));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);
    let mutation_runtime = SwarmMutationRuntime::default();

    handle_comm_task_control(
        101,
        requester.to_string(),
        "wake".to_string(),
        "active-task".to_string(),
        Some(worker.to_string()),
        Some("continue".to_string()),
        &client_tx,
        &sessions,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &swarm_plans,
        &swarm_coordinators,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &mutation_runtime,
    )
    .await;

    match client_rx.recv().await.expect("response") {
        ServerEvent::CommTaskControlResponse {
            id,
            action,
            task_id,
            target_session,
            status,
            summary,
        } => {
            assert_eq!(id, 101);
            assert_eq!(action, "wake");
            assert_eq!(task_id, "active-task");
            assert_eq!(target_session.as_deref(), Some(worker));
            assert_eq!(status, "running");
            assert_eq!(summary.item_count, 2);
            assert!(summary.ready_ids.contains(&"next".to_string()));
        }
        other => panic!("expected CommTaskControlResponse, got {other:?}"),
    }
}
