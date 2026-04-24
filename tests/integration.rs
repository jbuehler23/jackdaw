use std::marker::PhantomData;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::util::OperatorResultExt as _;
mod util;

#[test]
fn run_integration_tests() {
    // TODO: this integration test setup would be great for extension writers too, but it requires quite a bit of boilerplate to get there.
    // we should probably provide a macro that sets up everything under a neat `#[test]` for you, so you can directly write your test operator.
    run_test::<IntegrationTestOneOp>();
}

#[operator(id = ID)]
fn integration_test_one(_: In<OperatorParameters>) -> OperatorResult {
    // todo: fill in an actual test and write more!
    OperatorResult::Finished
}

fn run_test<T: Operator + Send + Sync>() {
    let mut app = util::headless_app();
    util::register_and_enable_extension::<IntegrationTestsExtension<T>>(&mut app);
    app.world_mut()
        .operator(ID)
        .call()
        .unwrap()
        .assert_finished();
}

pub struct IntegrationTestsExtension<T: Operator>(PhantomData<T>);
impl<T: Operator> Default for IntegrationTestsExtension<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: Operator + Send + Sync> JackdawExtension for IntegrationTestsExtension<T> {
    fn id(&self) -> String {
        "Integration Tests".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<T>();
    }
}

const ID: &str = "integration_test.run_test";
