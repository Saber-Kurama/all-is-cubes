#![no_main]
#![allow(clippy::single_match)]

use libfuzzer_sys::fuzz_target;

extern crate all_is_cubes;
use all_is_cubes::block::Block;

fuzz_target!(|block: Block| {
    // TODO: The `Block` will have pending URefs (not inserted in a Universe).
    // Put them in a universe once that is possible.

    let sink = all_is_cubes::listen::Sink::new();

    match block.evaluate_and_listen(sink.listener()) {
        Ok(evaluated) => {
            evaluated.consistency_check();
        }
        Err(_) => {
            // Errors are an expected possibility; this fuzz test is looking for no panic.
        }
    }

    // Exercise visit_refs(), because it is a recursive operation on the block + modifiers.
    // TODO: To do this we need a handy visitor, but I think it should be simplified...
    // block.visit_refs(visitor);
});
