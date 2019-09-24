use crate::{Net, Node, Spec};
use ckb_types::core::{BlockView, EpochNumberWithFraction};
use ckb_types::prelude::*;
use log::info;

// Convention:
//   main-block: a block on the main fork
//   main-uncle: an uncle block be embedded in the main fork
//   fork-block: a block on a side fork
//   fork-uncle: an uncle block be embedded in a side fork

pub struct UncleInheritFromForkBlock;

impl Spec for UncleInheritFromForkBlock {
    crate::name!("uncle_inherit_from_fork_block");

    crate::setup!(num_nodes: 2, connect_all: false);

    // Case: A uncle inherited from a fork-block in side fork is invalid, because that breaks
    //       the uncle rule "B1's parent is either B2's ancestor or embedded in B2 or its ancestors
    //       as an uncle"
    fn run(&self, net: Net) {
        let target_node = &net.nodes[0];
        let feed_node = &net.nodes[1];

        info!("(1) Build a chain which embedded `uncle` as an uncle");
        let uncle = construct_uncle(target_node);

        info!("(2) Force reorg, so that the parent of `uncle` become fork-block");
        let longer_fork = (0..=target_node.get_tip_block_number()).map(|_| {
            let block = feed_node.new_block(None, None, None);
            feed_node.submit_block(&block.data());
            block
        });
        longer_fork.for_each(|block| {
            target_node.submit_block(&block.data());
        });

        info!(
            "(3) Submit block with `uncle`, which is inherited from a fork-block, should be failed"
        );
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .build();
        let ret = target_node
            .rpc_client()
            .submit_block("0".to_owned(), block.data().into());
        assert!(ret.is_err(), "{:?}", ret);
        let err = ret.unwrap_err();
        assert!(err.to_string().contains("DescendantLimit"), "{:?}", err);

        info!("(4) Add all the fork-blocks as uncle blocks into the chain. And now re-submit block with `uncle` should be success");
        until_no_uncles_left(target_node);
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .build();
        target_node.submit_block(&block.data());
    }
}

pub struct UncleInheritFromForkUncle;

impl Spec for UncleInheritFromForkUncle {
    crate::name!("uncle_inherit_from_fork_uncle");

    crate::setup!(num_nodes: 2, connect_all: false);

    // Case: A uncle inherited from a fork-uncle in side fork is invalid, because that breaks
    //       the uncle rule "B1's parent is either B2's ancestor or embedded in B2 or its ancestors
    //       as an uncle"
    fn run(&self, net: Net) {
        let target_node = &net.nodes[0];
        let feed_node = &net.nodes[1];
        net.exit_ibd_mode();

        info!("(1) Build a chain which embedded `uncle_parent` as an uncle");
        let uncle_parent = construct_uncle(target_node);
        target_node.submit_block(&uncle_parent.data());

        let uncle_child = uncle_parent
            .as_advanced_builder()
            .number((uncle_parent.number() + 1).pack())
            .parent_hash(uncle_parent.hash())
            .timestamp((uncle_parent.timestamp() + 1).pack())
            .epoch(
                {
                    let parent_epoch = uncle_parent.epoch();
                    let epoch_number = parent_epoch.number();
                    let epoch_index = parent_epoch.index();
                    let epoch_length = parent_epoch.length();
                    EpochNumberWithFraction::new(epoch_number, epoch_index + 1, epoch_length)
                }
                .pack(),
            )
            .build();

        info!("(2) Force reorg, so that `uncle_parent` become a fork-uncle");
        let longer_fork = (0..=target_node.get_tip_block_number()).map(|_| {
            let block = feed_node.new_block(None, None, None);
            feed_node.submit_block(&block.data());
            block
        });
        longer_fork.for_each(|block| {
            target_node.submit_block(&block.data());
        });

        info!("(3) Submit block with `uncle`, which is inherited from fork-uncle `uncle_parent`, should be failed");
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle_child.as_uncle()])
            .build();
        let ret = target_node
            .rpc_client()
            .submit_block("0".to_owned(), block.data().into());
        assert!(ret.is_err(), "{:?}", ret);
        let err = ret.unwrap_err();
        assert!(err.to_string().contains("DescendantLimit"), "{:?}", err);

        info!("(4) Add all the fork-blocks as uncle blocks into the chain. And now re-submit block with `uncle_child` should be success");
        until_no_uncles_left(target_node);
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle_child.as_uncle()])
            .build();
        target_node.submit_block(&block.data());
    }
}

pub struct PackUnclesIntoEpochStarting;

impl Spec for PackUnclesIntoEpochStarting {
    crate::name!("pack_uncles_into_epoch_starting");

    // Case: Miner should not add uncles into the epoch starting
    fn run(&self, net: Net) {
        let node = &net.nodes[0];
        let uncle = construct_uncle(node);
        let next_epoch_start = {
            let current_epoch = node.rpc_client().get_current_epoch();
            current_epoch.start_number.value() + current_epoch.length.value()
        };
        let current_epoch_end = next_epoch_start - 1;

        info!(
            "(1) Grow until CURRENT_EPOCH_END-1({})",
            current_epoch_end - 1
        );
        node.generate_blocks((current_epoch_end - node.get_tip_block_number() - 1) as usize);
        assert_eq!(current_epoch_end - 1, node.get_tip_block_number());

        info!("(2) Submit the target uncle");
        node.submit_block(&uncle.data());

        info!("(3) Expect the next mining block(CURRENT_EPOCH_END) contains the target uncle");
        let block = node.new_block(None, None, None);
        assert_eq!(1, block.uncles_count());

        // Clear the uncles in the next block, we don't want to pack the target `uncle` now.
        info!("(4) Submit the next block with empty uncles");
        let block_with_empty_uncles = block.as_advanced_builder().set_uncles(vec![]).build();
        node.submit_block(&block_with_empty_uncles.data());

        info!("(5) Expect the next mining block(NEXT_EPOCH_START) not contains the target uncle");
        let block = node.new_block(None, None, None);
        assert_eq!(0, block.uncles_count());
    }
}

// Convenient way to construct an uncle block
fn construct_uncle(node: &Node) -> BlockView {
    node.generate_block(); // Ensure exit IBD mode

    let block = node.new_block(None, None, None);
    let timestamp = block.timestamp() + 10;
    let uncle = block
        .as_advanced_builder()
        .timestamp(timestamp.pack())
        .build();

    node.generate_block();

    uncle
}

// Convenient way to add all the fork-blocks as uncles into the main chain
fn until_no_uncles_left(node: &Node) {
    loop {
        let block = node.new_block(None, None, None);
        if block.uncles_count() == 0 {
            break;
        }
        node.submit_block(&block.data());
    }
}
