use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;

use crate::{f_graph::HashableGraphNode, runner::TaskRunner};

mod f_graph;
mod runner;

#[tokio::main]
async fn main() -> Result<()> {
    let mut runner = TaskRunner::new().with_concurrency(3);

    let t1 = HashableGraphNode::new(1, Vec::new(), || {
        Box::pin(async {
            println!("t1 start");
            sleep(Duration::from_millis(200)).await; // shorter
            println!("t1 done");
            Ok(())
        })
    });
    let t1_hash = t1.hash;
    let t2 = HashableGraphNode::new(2, vec![t1_hash], || {
        Box::pin(async {
            println!("t2 start");
            sleep(Duration::from_millis(200)).await;
            println!("t2 done");
            Ok(())
        })
    });

    let t3 = HashableGraphNode::new(1, vec![t1_hash], || {
        Box::pin(async {
            println!("t3 start");
            sleep(Duration::from_millis(200)).await;
            println!("t3 done");
            Ok(())
        })
    });
    runner.add_task(t1)?;
    runner.add_task(t2)?;
    runner.add_task(t3)?;

    runner.run_all().await?;

    println!("All tasks completed");

    let t4 = HashableGraphNode::new(10, vec![t1_hash], || {
        Box::pin(async {
            println!("t4 start");
            sleep(Duration::from_millis(200)).await;
            println!("t4 done");
            Ok(())
        })
    });

    let t5 = HashableGraphNode::new(15, vec![t1_hash], || {
        Box::pin(async {
            println!("t5 start");
            sleep(Duration::from_millis(200)).await;
            println!("t5 done");
            Ok(())
        })
    });

    runner.add_task(t4)?;
    runner.add_task(t5)?;

    runner.run_all().await?;
    println!("All tasks completed again");

    Ok(())
}
