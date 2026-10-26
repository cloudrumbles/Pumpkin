use super::schedule::{BACKGROUND_PRIORITY, Priorities, TaskHeapNode, TaskQueue, retry_priority};
use super::*;
use crate::chunk_system::dag::Node;
use crate::chunk_system::dag::NodeKey;
use slotmap::Key;
use std::collections::HashMap;

#[test]
fn ensure_dependency_chain_builds_multistage_chain() {
    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let priorities = Priorities::default();

    let chunk_pos = ChunkPos::new(0, 0);

    // Create a dependency node in the graph which will depend on the chain
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        ..Default::default()
    };

    // Build a chain up to Surface (Empty -> ... -> Surface)
    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &priorities,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    let start = (holder.current_stage as usize + 1).max(StagedChunkEnum::Empty as usize);
    let end = StagedChunkEnum::Surface as u8 as usize;

    // Ensure tasks were created, present in the DAG, and correctly chained
    for idx in start..=end {
        let key = holder.tasks[idx];
        assert!(!key.is_null(), "task {idx} was not created");

        let node = graph.nodes.get(key).expect("graph missing node");

        if idx == start {
            assert_eq!(node.in_degree, 0, "Start task should have 0 in_degree");
        } else {
            assert_eq!(
                node.in_degree, 1,
                "Intermediate task {idx} should have in_degree of 1"
            );
        }
    }

    // The dependency node should have its in_degree incremented
    let dep_node = graph.nodes.get(dependency_task).unwrap();
    assert_eq!(dep_node.in_degree, 1);

    // The entry task should have been queued
    let queued = queue.pop().expect("queue should have entry task");
    assert_eq!(queued.node_key(), holder.tasks[start]);
}

#[test]
fn ensure_dependency_chain_resumes_partial_chain() {
    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let priorities = Priorities::default();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Biomes,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &priorities,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    // Dynamically calculate the next stage after Biomes instead of guessing
    let empty = StagedChunkEnum::Empty as usize;
    let start = (holder.current_stage as usize + 1).max(empty);

    let queued = queue.pop().expect("queue should have entry task");
    assert_eq!(
        queued.node_key(),
        holder.tasks[start],
        "Should resume directly from the next stage after Biomes"
    );

    let entry_node = graph.nodes.get(queued.node_key()).unwrap();
    assert_eq!(
        entry_node.in_degree, 0,
        "Resumed task should have 0 in_degree because previous stages are already done"
    );
}

#[test]
fn ensure_dependency_chain_does_nothing_if_already_met() {
    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let priorities = Priorities::default();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Full,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &priorities,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface, // Requesting a lower stage than it currently is
    );

    // Ensure the function returned early without creating any tasks or queueing anything
    for task in &holder.tasks {
        assert!(
            task.is_null(),
            "No tasks should be created if the stage requirement is already met"
        );
    }
    assert!(queue.is_empty(), "Nothing should be queued");
}

#[test]
fn ensure_dependency_chain_respects_occupied_lock() {
    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let priorities = Priorities::default();

    let chunk_pos = ChunkPos::new(0, 0);
    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(10, 10), StagedChunkEnum::Features));

    // Create an "occupy" node to simulate another thread/process currently working on this chunk
    let occupy_node = graph.nodes.insert(Node::new(
        ChunkPos::new(i32::MAX, i32::MAX),
        StagedChunkEnum::None,
    ));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        occupied: occupy_node,
        ..Default::default()
    };

    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &priorities,
        dependency_task,
        chunk_pos,
        &mut holder,
        StagedChunkEnum::Surface,
    );

    let start = StagedChunkEnum::Empty as usize;
    let entry_task = holder.tasks[start];

    let entry_node = graph.nodes.get(entry_task).unwrap();
    // The first task should depend on the occupy node finishing!
    assert_eq!(
        entry_node.in_degree, 1,
        "Entry task should be blocked by the occupy node"
    );

    // Therefore, the queue MUST be empty. It shouldn't fire until the occupy node drops.
    assert!(
        queue.is_empty(),
        "Task should not be queued because it is blocked by occupied status"
    );
}

#[test]
fn ensure_dependency_chain_early_return_skips_edge() {
    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let priorities = Priorities::default();

    let dependency_task = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 1), StagedChunkEnum::Surface));

    // Create a holder that is already at the required stage (Features > Surface)
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Features,
        target_stage: StagedChunkEnum::Features,
        ..Default::default()
    };

    // Require 'Empty', but holder is already at 'Features'
    GenerationSchedule::ensure_dependency_chain(
        &mut graph,
        &mut queue,
        &priorities,
        dependency_task,
        ChunkPos::new(0, 0),
        &mut holder,
        StagedChunkEnum::Empty,
    );

    let dep_node = graph.nodes.get(dependency_task).unwrap();

    // Because of the early return, the dependency_task should not have its in_degree increased
    assert_eq!(
        dep_node.in_degree, 0,
        "Dependency task should not be blocked if the neighbor is already past the required stage"
    );
}

#[test]
fn completed_proto_stage_drops_all_satisfied_tasks() {
    let mut graph = DAG::default();
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::None,
        target_stage: StagedChunkEnum::StructureStart,
        ..Default::default()
    };

    for stage in StagedChunkEnum::Empty as usize..=StagedChunkEnum::StructureStart as usize {
        holder.tasks[stage] = graph.nodes.insert(Node::new(
            ChunkPos::new(0, 0),
            StagedChunkEnum::from(stage as u8),
        ));
    }

    for stage in StagedChunkEnum::Empty as usize..StagedChunkEnum::StructureStart as usize {
        graph.add_edge(holder.tasks[stage], holder.tasks[stage + 1]);
    }

    let returned_stage = StagedChunkEnum::Biomes as usize;
    for task_idx in
        (holder.current_stage as usize + 1)..=(returned_stage).min(holder.tasks.len() - 1)
    {
        if !holder.tasks[task_idx].is_null() {
            if let Some(old) = graph.nodes.remove(holder.tasks[task_idx]) {
                let mut edge = old.edge;
                while !edge.is_null() {
                    let cur = graph.edges.remove(edge).unwrap();
                    if let Some(node) = graph.nodes.get_mut(cur.to) {
                        node.in_degree -= 1;
                    }
                    edge = cur.next;
                }
            }
            holder.tasks[task_idx] = NodeKey::null();
        }
    }

    assert!(holder.tasks[StagedChunkEnum::Empty as usize].is_null());
    assert!(holder.tasks[StagedChunkEnum::Biomes as usize].is_null());
    assert!(!holder.tasks[StagedChunkEnum::StructureStart as usize].is_null());
    assert!(
        graph
            .nodes
            .get(holder.tasks[StagedChunkEnum::StructureStart as usize])
            .is_some()
    );
}

#[test]
fn cancellation_path_decrements_in_degree() {
    let mut graph = DAG::default();

    // Create a task that is waiting (in_degree = 1)
    let waiting_task_key = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Surface));
    let waiting_node = graph.nodes.get_mut(waiting_task_key).unwrap();
    waiting_node.in_degree = 1;

    // Create the occupy node that the task is waiting on
    let occupy_key = graph.nodes.insert(Node::new(
        ChunkPos::new(i32::MAX, i32::MAX),
        StagedChunkEnum::None,
    ));
    graph.add_edge(occupy_key, waiting_task_key);

    // Set up the chunk map to simulate an in-flight task
    let mut chunk_map = HashMap::new();
    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Empty,
        target_stage: StagedChunkEnum::Surface,
        occupied: occupy_key,
        ..Default::default()
    };
    holder.tasks[StagedChunkEnum::Surface as usize] = waiting_task_key;
    chunk_map.insert(ChunkPos::new(0, 0), holder);

    // Simulate the cancellation logic from `work()`
    let mut nodes_to_drop = Vec::new();
    for holder in chunk_map.values_mut() {
        for task in &mut holder.tasks {
            if !task.is_null() {
                nodes_to_drop.push(*task);
                *task = NodeKey::null();
            }
        }
        if !holder.occupied.is_null() {
            nodes_to_drop.push(holder.occupied);
            holder.occupied = NodeKey::null();
        }
    }

    // Drop nodes using the proper graph edge traversal
    for node_key in nodes_to_drop {
        // Simulating the self.drop_node logic inside the test
        if let Some(old) = graph.nodes.remove(node_key) {
            let mut edge = old.edge;
            while !edge.is_null() {
                let cur = graph.edges.remove(edge).unwrap();
                if let Some(node) = graph.nodes.get_mut(cur.to) {
                    node.in_degree -= 1;
                }
                edge = cur.next;
            }
        }
    }

    // Assert that the waiting task was properly cleaned up
    assert!(
        graph.nodes.get(waiting_task_key).is_none(),
        "The waiting task should have been dropped during cancellation"
    );
}

#[test]
fn dag_drop_edge_chain_removes_all_edges() {
    let mut graph = DAG::default();
    let node1 = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Empty));
    let node2 = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 0), StagedChunkEnum::Empty));

    let edge1 = graph.edges.insert(crate::chunk_system::dag::Edge::new(
        node1,
        crate::chunk_system::dag::EdgeKey::null(),
    ));
    let edge2 = graph
        .edges
        .insert(crate::chunk_system::dag::Edge::new(node2, edge1));

    assert_eq!(graph.edges.len(), 2);
    graph.drop_edge_chain(edge2);
    assert_eq!(graph.edges.len(), 0);
}

#[test]
fn dag_prune_edge_chain_removes_dead_target_edges() {
    let mut graph = DAG::default();
    let node_alive = graph
        .nodes
        .insert(Node::new(ChunkPos::new(0, 0), StagedChunkEnum::Empty));
    let node_dead = graph
        .nodes
        .insert(Node::new(ChunkPos::new(1, 0), StagedChunkEnum::Empty));

    let edge_alive = graph.edges.insert(crate::chunk_system::dag::Edge::new(
        node_alive,
        crate::chunk_system::dag::EdgeKey::null(),
    ));
    let edge_dead = graph
        .edges
        .insert(crate::chunk_system::dag::Edge::new(node_dead, edge_alive));

    // Remove node_dead from graph.nodes to simulate completed/dropped task
    graph.nodes.remove(node_dead);

    let mut head = edge_dead;
    let has_valid_task = graph.prune_edge_chain(&mut head);

    assert!(has_valid_task);
    assert_eq!(head, edge_alive);
    assert_eq!(graph.edges.len(), 1);
    assert!(graph.edges.contains_key(edge_alive));
    assert!(!graph.edges.contains_key(edge_dead));

    // Now remove node_alive as well
    graph.nodes.remove(node_alive);
    let has_valid_task = graph.prune_edge_chain(&mut head);
    assert!(!has_valid_task);
    assert!(head.is_null());
    assert_eq!(graph.edges.len(), 0);
}

#[test]
fn cancelled_out_of_range_task_clears_holder_task_slot() {
    let mut graph = DAG::default();
    let mut chunk_map = HashMap::new();
    let pos = ChunkPos::new(0, 0);

    let stage = StagedChunkEnum::Biomes;
    let node_key = graph.nodes.insert(Node::new(pos, stage));

    let mut holder = ChunkHolder {
        current_stage: StagedChunkEnum::Empty,
        target_stage: StagedChunkEnum::None, // Chunk was downgraded / moved out of range
        dependency_stage: StagedChunkEnum::None,
        ..Default::default()
    };
    holder.tasks[stage as usize] = node_key;
    chunk_map.insert(pos, holder);

    // Simulate task processing loop cancellation when node.stage > effective_target
    let node = graph.nodes.get(node_key).unwrap().clone();
    let effective_target = chunk_map.get(&node.pos).map_or(StagedChunkEnum::None, |h| {
        h.target_stage.max(h.dependency_stage)
    });

    assert!(node.stage > effective_target);
    if let Some(holder) = chunk_map.get_mut(&node.pos) {
        let task_slot = &mut holder.tasks[node.stage as usize];
        if *task_slot == node_key {
            *task_slot = NodeKey::null();
        }
    }
    graph.fast_drop_node(node_key);

    let holder = chunk_map.get(&pos).unwrap();
    assert!(
        holder.tasks[stage as usize].is_null(),
        "Task slot must be null after cancellation"
    );
    assert!(
        holder.tasks.iter().all(Key::is_null),
        "All task slots must be null when idle"
    );
}

fn priorities_with_player_at_origin() -> Priorities {
    // A player with view distance 10 holds a ticket at level 43 - 11 = 32.
    Priorities {
        foreground: vec![(ChunkPos::new(0, 0), 32)],
        ..Priorities::default()
    }
}

#[test]
fn foreground_reach_ends_one_ring_beyond_the_loaded_area() {
    let priorities = priorities_with_player_at_origin();
    // Level 32 + 17 = 49 is the last ring a dependency can reach.
    assert!(!priorities.is_background(ChunkPos::new(17, -3)));
    assert!(priorities.is_background(ChunkPos::new(18, 0)));
    assert!(Priorities::default().is_background(ChunkPos::new(0, 0)));
}

#[test]
fn background_tasks_run_after_every_foreground_task() {
    let mut priorities = priorities_with_player_at_origin();
    priorities.level.insert(ChunkPos::new(17, 0), 49);
    priorities
        .level
        .insert(ChunkPos::new(100, 0), ChunkLoading::FULL_CHUNK_LEVEL);

    let mut graph = DAG::default();
    let mut queue = TaskQueue::default();
    let far_empty = graph
        .nodes
        .insert(Node::new(ChunkPos::new(100, 0), StagedChunkEnum::Empty));
    let near_full = graph
        .nodes
        .insert(Node::new(ChunkPos::new(17, 0), StagedChunkEnum::Noise));
    queue.push(TaskHeapNode(
        priorities.of(ChunkPos::new(100, 0), StagedChunkEnum::Empty),
        far_empty,
    ));
    queue.push(TaskHeapNode(
        priorities.of(ChunkPos::new(17, 0), StagedChunkEnum::Noise),
        near_full,
    ));

    assert_eq!(queue.foreground.len(), 1);
    assert_eq!(queue.background.len(), 1);
    assert_eq!(queue.pop().unwrap().node_key(), near_full);
    assert_eq!(queue.pop().unwrap().node_key(), far_empty);
}

#[test]
fn a_failed_background_task_is_retried_as_background() {
    let priorities = Priorities::default();
    let priority = priorities.of(ChunkPos::new(0, 0), StagedChunkEnum::Empty);
    assert!(retry_priority(priority) >= BACKGROUND_PRIORITY);
}

#[test]
fn later_background_stages_run_first() {
    let mut priorities = Priorities::default();
    priorities
        .level
        .insert(ChunkPos::new(0, 0), ChunkLoading::FULL_CHUNK_LEVEL);
    priorities.level.insert(ChunkPos::new(1, 0), 44);
    let pos = ChunkPos::new(0, 0);
    let lighting = priorities.of(pos, StagedChunkEnum::Lighting);
    let noise = priorities.of(pos, StagedChunkEnum::Noise);
    assert!(lighting < noise);
    // At the same stage, the ticketed chunk runs before its neighbour.
    assert!(noise < priorities.of(ChunkPos::new(1, 0), StagedChunkEnum::Noise));
}
