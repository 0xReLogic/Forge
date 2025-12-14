use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct Stage {
    name: String,
    depends_on: Vec<String>,
}

fn resolve_stage_dependencies(stages: &[Stage]) -> Result<Vec<String>, String> {
    let stage_names: HashSet<String> = stages.iter().map(|s| s.name.clone()).collect();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for stage in stages {
        graph.entry(stage.name.clone()).or_default();
        in_degree.entry(stage.name.clone()).or_insert(0);

        for dep in &stage.depends_on {
            if dep == &stage.name {
                return Err(format!("Self dependency: {}", stage.name));
            }
            if !stage_names.contains(dep) {
                return Err(format!("Missing dependency: {} -> {}", stage.name, dep));
            }
            graph
                .entry(dep.clone())
                .or_default()
                .push(stage.name.clone());
            *in_degree.entry(stage.name.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    queue.sort();

    let mut result = Vec::new();
    while let Some(current) = queue.pop() {
        result.push(current.clone());
        if let Some(dependents) = graph.get(&current) {
            for dependent in dependents {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dependent.clone());
                        queue.sort();
                    }
                }
            }
        }
    }

    if result.len() != stages.len() {
        return Err("Circular dependency detected".to_string());
    }
    Ok(result)
}

#[test]
fn test_simple_dependency_chain() {
    let stages = vec![
        Stage {
            name: "setup".to_string(),
            depends_on: vec![],
        },
        Stage {
            name: "build".to_string(),
            depends_on: vec!["setup".to_string()],
        },
        Stage {
            name: "test".to_string(),
            depends_on: vec!["build".to_string()],
        },
    ];
    let result = resolve_stage_dependencies(&stages).unwrap();
    assert_eq!(result, vec!["setup", "build", "test"]);
}

#[test]
fn test_complex_dependency_chain() {
    let stages = vec![
        Stage {
            name: "setup".to_string(),
            depends_on: vec![],
        },
        Stage {
            name: "build".to_string(),
            depends_on: vec!["setup".to_string()],
        },
        Stage {
            name: "test".to_string(),
            depends_on: vec!["build".to_string()],
        },
        Stage {
            name: "package".to_string(),
            depends_on: vec!["build".to_string(), "test".to_string()],
        },
        Stage {
            name: "deploy".to_string(),
            depends_on: vec!["package".to_string()],
        },
    ];
    let result = resolve_stage_dependencies(&stages).unwrap();

    // Verify order respects dependencies
    let pos: HashMap<&str, usize> = result
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    assert!(pos["setup"] < pos["build"]);
    assert!(pos["build"] < pos["test"]);
    assert!(pos["build"] < pos["package"]);
    assert!(pos["test"] < pos["package"]);
    assert!(pos["package"] < pos["deploy"]);
}

#[test]
fn test_circular_dependency() {
    let stages = vec![
        Stage {
            name: "build".to_string(),
            depends_on: vec!["test".to_string()],
        },
        Stage {
            name: "test".to_string(),
            depends_on: vec!["build".to_string()],
        },
    ];
    let result = resolve_stage_dependencies(&stages);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Circular"));
}

#[test]
fn test_self_dependency() {
    let stages = vec![Stage {
        name: "build".to_string(),
        depends_on: vec!["build".to_string()],
    }];
    let result = resolve_stage_dependencies(&stages);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Self dependency"));
}

#[test]
fn test_missing_dependency() {
    let stages = vec![Stage {
        name: "build".to_string(),
        depends_on: vec!["nonexistent".to_string()],
    }];
    let result = resolve_stage_dependencies(&stages);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Missing dependency"));
}

#[test]
fn test_no_dependencies() {
    let stages = vec![
        Stage {
            name: "a".to_string(),
            depends_on: vec![],
        },
        Stage {
            name: "b".to_string(),
            depends_on: vec![],
        },
        Stage {
            name: "c".to_string(),
            depends_on: vec![],
        },
    ];
    let result = resolve_stage_dependencies(&stages).unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn test_diamond_dependency() {
    // A -> B, A -> C, B -> D, C -> D
    let stages = vec![
        Stage {
            name: "a".to_string(),
            depends_on: vec![],
        },
        Stage {
            name: "b".to_string(),
            depends_on: vec!["a".to_string()],
        },
        Stage {
            name: "c".to_string(),
            depends_on: vec!["a".to_string()],
        },
        Stage {
            name: "d".to_string(),
            depends_on: vec!["b".to_string(), "c".to_string()],
        },
    ];
    let result = resolve_stage_dependencies(&stages).unwrap();

    let pos: HashMap<&str, usize> = result
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    assert!(pos["a"] < pos["b"]);
    assert!(pos["a"] < pos["c"]);
    assert!(pos["b"] < pos["d"]);
    assert!(pos["c"] < pos["d"]);
}

#[test]
fn test_three_way_circular() {
    let stages = vec![
        Stage {
            name: "a".to_string(),
            depends_on: vec!["c".to_string()],
        },
        Stage {
            name: "b".to_string(),
            depends_on: vec!["a".to_string()],
        },
        Stage {
            name: "c".to_string(),
            depends_on: vec!["b".to_string()],
        },
    ];
    let result = resolve_stage_dependencies(&stages);
    assert!(result.is_err());
}
