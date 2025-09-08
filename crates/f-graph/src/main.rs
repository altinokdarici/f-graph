use std::time::Duration;

use anyhow::Result;
use f_graph::{FGraph, GraphNode};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<()> {
    let mut runner = FGraph::new().with_concurrency(3);

    let t1 = GraphNode::new(1, Vec::new(), || {
        Box::pin(async {
            println!("t1 start");
            sleep(Duration::from_millis(200)).await; // shorter
            println!("t1 done");
            Ok(vec![])
        })
    });
    let t1_index = runner.add_task(t1)?;

    let t2 = GraphNode::new(2, vec![t1_index], || {
        Box::pin(async {
            println!("t2 start");
            sleep(Duration::from_millis(200)).await;
            println!("t2 done");
            Ok(vec![])
        })
    });
    runner.add_task(t2)?;

    let t3 = GraphNode::new(1, vec![t1_index], || {
        Box::pin(async {
            println!("t3 start");
            sleep(Duration::from_millis(200)).await;
            println!("t3 done");
            Ok(vec![])
        })
    });
    runner.add_task(t3)?;

    runner.run_all().await?;

    println!("All tasks completed");

    let t4 = GraphNode::new(10, vec![t1_index], || {
        Box::pin(async {
            println!("t4 start");
            sleep(Duration::from_millis(200)).await;
            println!("t4 done");
            Ok(vec![])
        })
    });

    let t5 = GraphNode::new(15, vec![t1_index], || {
        Box::pin(async {
            println!("t5 start");
            sleep(Duration::from_millis(200)).await;
            println!("t5 done");
            Ok(vec![])
        })
    });

    runner.add_task(t4)?;
    runner.add_task(t5)?;

    runner.run_all().await?;
    println!("All tasks completed again");

    Ok(())
}
