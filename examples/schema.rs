use banana_vault::msg::{ExecuteMsg, InstantiateMsg, QueryMsg, MigrateMsg};
use cosmwasm_schema::write_api;

//run cargo schema to generate
fn main() {
    write_api! {
        instantiate: InstantiateMsg,
        execute: ExecuteMsg,
        query: QueryMsg,
        migrate: MigrateMsg,
    }
}
