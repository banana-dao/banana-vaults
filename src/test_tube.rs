use crate::msg::{
    AccountQuery, AccountQueryParams, AccountResponse, CancelMsg, DepositMsg, Environment,
    ExecuteMsg::{Cancel, Deposit, ManagePosition, ManageVault},
    InstantiateMsg,
    PositionMsg::{CreatePosition, WithdrawPosition},
    QueryMsg::{AccountStatus, LockedAssets},
    VaultAsset, VaultMsg,
};
use cosmos_sdk_proto::cosmos::params::v1beta1::ParameterChangeProposal;
use cosmwasm_std::{coin, Addr, Coin, Coins, Decimal, Uint128};
use osmosis_std::types::{
    cosmos::bank::v1beta1::{MsgSend, QueryTotalSupplyRequest},
    osmosis::concentratedliquidity::v1beta1::{MsgCreatePosition, UserPositionsRequest},
};
use osmosis_test_tube::osmosis_std::types::cosmos::bank::v1beta1::QueryBalanceRequest;
use osmosis_test_tube::{
    osmosis_std::types::osmosis::concentratedliquidity::poolmodel::concentrated::v1beta1::MsgCreateConcentratedPool,
    Account, Bank, ConcentratedLiquidity, FeeSetting, GovWithAppAccess, Module, OsmosisTestApp,
    SigningAccount, Wasm,
};
use pyth_sdk_cw::PriceIdentifier;
use std::ops::Div;
#[cfg(test)]
use std::ops::Mul;
struct TestEnv {
    app: OsmosisTestApp,
    contract_addr: String,
    admin: SigningAccount,
    users: Vec<SigningAccount>,
}

// arbitrary precision for comparing dollar amounts
// 99.9999% = 999999 / 1000000
const PRECISION: (Decimal, Decimal) = (
    Decimal::new(Uint128::new(999999)),
    Decimal::new(Uint128::new(1000000)),
);

const FEE_AMOUNT: u128 = 2500;
const TOTAL_FEES: u128 = FEE_AMOUNT * 20 * JOINS.len() as u128;

const JOINS: [([u128; 10], &str); 6] = [
    ([1, 1, 1, 1, 1, 1, 1, 1, 1, 1], "all small"),
    (
        [1_000_000, 1, 1, 1, 1, 1, 1, 1, 1, 1],
        "1 big and many small",
    ),
    (
        [
            1, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
            1_000_000, 1_000_000,
        ],
        "one small many big",
    ),
    (
        [
            1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
            1_000_000, 1_000_000,
        ],
        " all big",
    ),
    (
        [3, 18, 1821, 7134, 82, 90111, 237, 529, 7821, 52121],
        "random medium",
    ),
    (
        [29, 625, 3927, 6791, 2999, 95, 5, 664, 2341, 103],
        "random medium",
    ),
];

const WEI_JOINS: [([u128; 10], &str); 3] = [
    ([1, 1, 1, 1, 1, 1, 1, 1, 1, 1], "all small"),
    ([50, 1, 1, 1, 1, 1, 1, 1, 1, 1], "1 big and many small"),
    ([50, 50, 50, 50, 50, 50, 50, 50, 50, 50], "all big"),
];

struct Modules<'a> {
    bank: Bank<'a, OsmosisTestApp>,
    wasm: Wasm<'a, OsmosisTestApp>,
    cl: ConcentratedLiquidity<'a, OsmosisTestApp>,
    gov: GovWithAppAccess<'a>,
}

fn get_modules(test_env: &'_ TestEnv) -> Modules<'_> {
    Modules {
        bank: Bank::new(&test_env.app),
        wasm: Wasm::new(&test_env.app),
        cl: ConcentratedLiquidity::new(&test_env.app),
        gov: GovWithAppAccess::new(&test_env.app),
    }
}

fn setup_contract(asset1: VaultAsset) -> TestEnv {
    let app = OsmosisTestApp::new();

    let admin = app
        .init_account(&[
            Coin::new(1_000_000_000_000, asset1.denom.clone()),
            Coin::new(1_000_000_000_000, "uosmo"),
        ])
        .unwrap();

    let amount = 1_200_000 * 1_000_000 + TOTAL_FEES;
    let users: Vec<SigningAccount> = app
        .init_accounts(
            &[
                Coin::new(1_100_000 * 1_000_000, "uatom"),
                Coin::new(100 * 1_000_000_000_000_000_000, "wei"),
                Coin::new(amount, "uosmo"),
            ],
            10,
        )
        .unwrap();

    let new_fee_setting = FeeSetting::Custom {
        amount: coin(FEE_AMOUNT, "uosmo"),
        gas_limit: 1000000,
    };

    let updated_users: Vec<SigningAccount> = users
        .into_iter()
        .map(|user| user.with_fee_setting(new_fee_setting.clone()))
        .collect();

    let mut test_env = TestEnv {
        app,
        contract_addr: "".to_string(),
        admin,
        users: updated_users,
    };

    let modules = get_modules(&test_env);

    modules
        .gov
        .propose_and_execute(
            "/cosmos.params.v1beta1.ParameterChangeProposal".to_string(),
            ParameterChangeProposal {
                title: "test".to_string(),
                description: "test".to_string(),
                changes: vec![cosmos_sdk_proto::cosmos::params::v1beta1::ParamChange {
                    subspace: "concentratedliquidity".to_string(),
                    key: "UnrestrictedPoolCreatorWhitelist".to_string(),
                    value: format!("[\"{}\"]", test_env.admin.address().as_str()),
                }],
            },
            test_env.admin.address(),
            &test_env.admin,
        )
        .unwrap();

    modules
        .cl
        .create_concentrated_pool(
            MsgCreateConcentratedPool {
                denom0: "uosmo".to_string(),
                denom1: asset1.denom.clone(),
                sender: test_env.admin.address(),
                tick_spacing: 100,
                spread_factor: "0".to_string(),
            },
            &test_env.admin,
        )
        .unwrap();

    let denom_a = if "uosmo" < asset1.denom.as_str() {
        "uosmo"
    } else {
        asset1.denom.as_str()
    };

    let denom_b = if "uosmo" < asset1.denom.as_str() {
        asset1.denom.as_str()
    } else {
        "uosmo"
    };

    modules
        .cl
        .create_position(
            MsgCreatePosition {
                pool_id: 1,
                sender: test_env.admin.address(),
                lower_tick: -1000,
                upper_tick: 1000,
                tokens_provided: vec![coin(1000000, denom_a).into(), coin(1000000, denom_b).into()],
                token_min_amount0: "1".to_string(),
                token_min_amount1: "1".to_string(),
            },
            &test_env.admin,
        )
        .unwrap();

    let wasm_byte_code = std::fs::read("./target/wasm32-unknown-unknown/release/banana_vault.wasm")
        .unwrap_or_default();
    if wasm_byte_code.is_empty() {
        panic!("could not read wasm file - run `cargo wasm` first")
    }
    let code_id = modules
        .wasm
        .store_code(&wasm_byte_code, None, &test_env.admin)
        .unwrap()
        .data
        .code_id;

    let contract_addr = modules
        .wasm
        .instantiate(
            code_id,
            &InstantiateMsg {
                metadata: None,
                pool_id: 1,
                price_expiry: 60,
                min_asset0: 10000_u64.into(),
                min_asset1: 10000_u64.into(),
                asset0: VaultAsset {
                    denom: "uosmo".to_string(),
                    price_identifier: PriceIdentifier::from_hex(
                        "5867f5683c757393a0670ef0f701490950fe93fdb006d181c8265a831ac0c5c6",
                    )
                    .unwrap(),
                    decimals: 6,
                },
                asset1: asset1.clone(),
                min_redemption: None,
                dollar_cap: None,
                commission: Some(Decimal::from_ratio(1_u128, 100_u128)),
                commission_receiver: Some(Addr::unchecked(test_env.admin.address())),
                env: Some(Environment::Testtube),
                operator: Addr::unchecked(test_env.admin.address()),
            },
            Some(&test_env.admin.address()),
            Some("bv"),
            &[coin(100_000_000, "uosmo")],
            &test_env.admin,
        )
        .unwrap()
        .data
        .address;

    modules
        .bank
        .send(
            MsgSend {
                from_address: test_env.admin.address(),
                to_address: contract_addr.clone(),
                amount: vec![coin(1_000_000, asset1.denom.clone()).into()],
            },
            &test_env.admin,
        )
        .unwrap();

    test_env.contract_addr = contract_addr;
    test_env
}

fn get_asset(denom: &str) -> VaultAsset {
    match denom {
        "uatom" => VaultAsset {
            denom: "uatom".to_string(),
            price_identifier: PriceIdentifier::from_hex(
                "b00b60f88b03a6a625a8d1c048c3f66653edf217439983d037e7222c4e612819",
            )
            .unwrap(),
            decimals: 6,
        },
        "wei" => VaultAsset {
            denom: "wei".to_string(),
            price_identifier: PriceIdentifier::from_hex(
                "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace",
            )
            .unwrap(),
            decimals: 18,
        },
        _ => panic!("invalid denom"),
    }
}

fn get_price(denom: &str) -> Decimal {
    match denom {
        "uosmo" => Decimal::from_ratio(164243925_u128, 10_u64.pow(6)),
        "uatom" => Decimal::from_ratio(1031081328_u128, 10_u64.pow(6)),
        "wei" => Decimal::from_ratio(278558964008_u128, 10_u64.pow(18)),
        _ => panic!("invalid denom"),
    }
}

fn execute_joins(
    test_env: &TestEnv,
    modules: &Modules,
    join_amounts: ([u128; 10], &str),
    join_denom: &String,
    exp: u128,
) -> Vec<u128> {
    let mut initial_balances: Vec<u128> = vec![];

    for (i, user) in test_env.users.iter().enumerate() {
        // println!(
        //     "Joining user: {} with {} {}",
        //     i,
        //     join_amounts.0[i] * exp,
        //     join_denom
        // );

        let mut balance_after_fees: u128 = modules
            .bank
            .query_balance(&QueryBalanceRequest {
                address: user.address(),
                denom: join_denom.clone(),
            })
            .unwrap()
            .balance
            .unwrap()
            .amount
            .parse::<u128>()
            .unwrap();

        if join_denom == "uosmo" {
            balance_after_fees -= FEE_AMOUNT * 2;
        }

        initial_balances.push(balance_after_fees);

        modules
            .wasm
            .execute(
                &test_env.contract_addr,
                &Deposit(DepositMsg::Mint { min_out: None }),
                &[coin(join_amounts.0[i] * exp, join_denom)],
                user,
            )
            .unwrap();
    }
    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &ManageVault(VaultMsg::ProcessMints),
            &[],
            &test_env.admin,
        )
        .unwrap();

    initial_balances
}

fn execute_leaves(test_env: &TestEnv, modules: &Modules) {
    for user in &test_env.users {
        let bvt_balance = modules
            .bank
            .query_balance(&QueryBalanceRequest {
                address: user.address(),
                denom: format!("factory/{}/BVT", test_env.contract_addr),
            })
            .unwrap()
            .balance
            .unwrap()
            .amount
            .parse::<u128>()
            .unwrap();

        modules
            .wasm
            .execute(
                &test_env.contract_addr,
                &Deposit(DepositMsg::Burn {
                    address: None,
                    amount: None,
                }),
                &[coin(
                    bvt_balance,
                    format!("factory/{}/BVT", test_env.contract_addr),
                )],
                user,
            )
            .unwrap();
    }
}

fn user_balance_list(test_env: &TestEnv, modules: &Modules, denom: String) -> Vec<u128> {
    test_env
        .users
        .iter()
        .map(|user| {
            modules
                .bank
                .query_balance(&QueryBalanceRequest {
                    address: user.address(),
                    denom: denom.clone(),
                })
                .unwrap()
                .balance
                .unwrap()
                .amount
                .parse::<u128>()
                .unwrap()
        })
        .collect()
}

fn create_position(test_env: &TestEnv, modules: &Modules) {
    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &ManagePosition(CreatePosition {
                lower_tick: 2000,
                upper_tick: 3000,
                tokens_provided: vec![coin(1, "uosmo")],
                token_min_amount0: "0".to_string(),
                token_min_amount1: "0".to_string(),
                swap: None,
            }),
            &[],
            &test_env.admin,
        )
        .unwrap();
}

fn withdraw_position(update: bool, test_env: &TestEnv, modules: &Modules) {
    let position = modules
        .cl
        .query_user_positions(&UserPositionsRequest {
            address: test_env.contract_addr.clone(),
            pool_id: 1,
            pagination: None,
        })
        .unwrap()
        .positions[0]
        .clone()
        .position
        .unwrap();

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &ManagePosition(WithdrawPosition {
                position_id: position.position_id,
                liquidity_amount: position.liquidity,
                override_uptime: None,
            }),
            &[],
            &test_env.admin,
        )
        .unwrap();

    if update {
        modules
            .wasm
            .execute(
                &test_env.contract_addr,
                &ManageVault(VaultMsg::ProcessBurns),
                &[],
                &test_env.admin,
            )
            .unwrap();
    }
}

// withdraw and create position to trigger ratio calculation
fn cycle_positions(test_env: &TestEnv, modules: &Modules, exit: bool) {
    create_position(test_env, modules);
    withdraw_position(true, test_env, modules);
    create_position(test_env, modules);
    if exit {
        execute_leaves(test_env, modules);
    }
    withdraw_position(true, test_env, modules);
}

#[test]
fn test_instantiate() {
    setup_contract(get_asset("uatom"));
    setup_contract(get_asset("wei"));
}

#[test]
fn test_join_and_leave() {
    let test_env = setup_contract(get_asset("uatom"));
    let modules = get_modules(&test_env);

    for (i, join_amounts) in JOINS.iter().enumerate() {
        let initial_contract_balance: Vec<Coin> = modules
            .wasm
            .query(&test_env.contract_addr, &LockedAssets {})
            .unwrap();

        let initial_contract_dollar_value = initial_contract_balance[0]
            .amount
            .mul_floor(get_price("uosmo"))
            + initial_contract_balance[1]
                .amount
                .mul_floor(get_price("uatom"));

        let uosmo_initial_balances = execute_joins(
            &test_env,
            &modules,
            *join_amounts,
            &"uosmo".to_string(),
            1_000_000,
        );
        let uatom_initial_balances = execute_joins(
            &test_env,
            &modules,
            JOINS[JOINS.len() - (i + 1)],
            &"uatom".to_string(),
            1_000_000,
        );

        cycle_positions(&test_env, &modules, true);

        let uosmo_final_balances = user_balance_list(&test_env, &modules, "uosmo".to_string());
        let uatom_final_balances = user_balance_list(&test_env, &modules, "uatom".to_string());

        // println!("uosmo_initial_balances {:?}", uosmo_initial_balances);
        // println!("uosmo_final_balances {:?}", uosmo_final_balances);
        // println!("uatom_initial_balances {:?}", uatom_initial_balances);
        // println!("uatom_final_balances {:?}", uatom_final_balances);

        // get dollar amounts of initial and final balances
        let mut initial_dollar_value = vec![];

        for balance in uosmo_initial_balances.iter() {
            initial_dollar_value.push(Decimal::new(Uint128::new(*balance)).mul(get_price("uosmo")));
        }

        for (i, balance) in uatom_initial_balances.iter().enumerate() {
            initial_dollar_value[i] += Decimal::new(Uint128::new(*balance)).mul(get_price("uatom"));
        }

        let mut final_dollar_value = vec![];

        for balance in uosmo_final_balances.iter() {
            final_dollar_value.push(Decimal::new(Uint128::new(*balance)).mul(get_price("uosmo")));
        }

        for (i, balance) in uatom_final_balances.iter().enumerate() {
            final_dollar_value[i] += Decimal::new(Uint128::new(*balance)).mul(get_price("uatom"));
        }

        // println!("initial_dollar_value {:?}", initial_dollar_value);
        // println!("final_dollar_value {:?}", final_dollar_value);

        // we are comparing initial and final dollar amounts instead of discrete token amounts
        assert!(final_dollar_value
            .iter()
            .zip(initial_dollar_value.iter())
            .all(|(fin, initial)| fin >= &(initial.mul(PRECISION.0.div(PRECISION.1)))));

        // check that the contract balance is also correct
        let final_contract_balance: Vec<Coin> = modules
            .wasm
            .query(&test_env.contract_addr, &LockedAssets {})
            .unwrap();

        let final_dollar_value = final_contract_balance[0]
            .amount
            .mul_floor(get_price("uosmo"))
            + final_contract_balance[1]
                .amount
                .mul_floor(get_price("uatom"));

        assert!(
            final_dollar_value >= initial_contract_dollar_value.mul(PRECISION.0.div(PRECISION.1))
        );
    }
}

#[test]
fn test_join_and_leave_with_18_exp() {
    let test_env = setup_contract(get_asset("wei"));
    let modules = get_modules(&test_env);

    for (i, join_amounts) in WEI_JOINS.iter().enumerate() {
        let initial_contract_balance: Vec<Coin> = modules
            .wasm
            .query(&test_env.contract_addr, &LockedAssets {})
            .unwrap();

        let initial_contract_dollar_value = Decimal::new(initial_contract_balance[0].amount)
            .mul(get_price("uosmo"))
            + Decimal::new(initial_contract_balance[1].amount).mul(get_price("wei"));

        let uosmo_initial_balances = execute_joins(
            &test_env,
            &modules,
            JOINS[JOINS.len() - (i + 1)],
            &"uosmo".to_string(),
            1_000_000,
        );
        let wei_initial_balances = execute_joins(
            &test_env,
            &modules,
            *join_amounts,
            &"wei".to_string(),
            1_000_000_000_000_000_000,
        );

        cycle_positions(&test_env, &modules, true);

        let uosmo_final_balances = user_balance_list(&test_env, &modules, "uosmo".to_string());
        let wei_final_balances = user_balance_list(&test_env, &modules, "wei".to_string());

        let mut initial_dollar_value = vec![];

        for balance in uosmo_initial_balances.iter() {
            initial_dollar_value.push(Uint128::new(*balance).mul_floor(get_price("uosmo")));
        }

        for (i, balance) in wei_initial_balances.iter().enumerate() {
            initial_dollar_value[i] += Uint128::new(*balance).mul_floor(get_price("wei"));
        }

        let mut final_dollar_value = vec![];

        for balance in uosmo_final_balances.iter() {
            final_dollar_value.push(Uint128::new(*balance).mul_floor(get_price("uosmo")));
        }

        for (i, balance) in wei_final_balances.iter().enumerate() {
            final_dollar_value[i] += Uint128::new(*balance).mul_floor(get_price("wei"));
        }

        println!("initial_dollar_value {:?}", initial_dollar_value);
        println!("final_dollar_value {:?}", final_dollar_value);

        assert!(final_dollar_value
            .iter()
            .zip(initial_dollar_value.iter())
            .all(|(&fin, &initial)| Decimal::new(fin)
                >= Decimal::new(initial).mul(PRECISION.0.div(PRECISION.1))));

        let final_contract_balance: Vec<Coin> = modules
            .wasm
            .query(&test_env.contract_addr, &LockedAssets {})
            .unwrap();

        let final_dollar_value = Decimal::new(final_contract_balance[0].amount)
            .mul(get_price("uosmo"))
            + Decimal::new(final_contract_balance[1].amount).mul(get_price("wei"));

        assert!(
            final_dollar_value >= initial_contract_dollar_value.mul(PRECISION.0.div(PRECISION.1))
        );
    }
}

#[test]
fn test_multiple_recalculations() {
    let test_env = setup_contract(get_asset("uatom"));
    let modules = get_modules(&test_env);

    // make a new account for this test
    let u = test_env
        .app
        .init_account(&[
            Coin::new(100 * 1_000_000, "uatom"),
            Coin::new(1000 * 1_000_000, "uosmo"),
        ])
        .unwrap();

    let intial_usd = Decimal::new(Uint128::new(100 * 1_000_000)).mul(get_price("uatom"))
        + Decimal::new(Uint128::new(1000 * 1_000_000 - (FEE_AMOUNT * 2))).mul(get_price("uosmo"));

    println!("initial USD: {}", intial_usd);

    let user = u.with_fee_setting(FeeSetting::Custom {
        amount: coin(FEE_AMOUNT, "uosmo"),
        gas_limit: 1000000,
    });

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Mint { min_out: None }),
            &[coin(53_000_000, "uatom"), coin(500_000_000, "uosmo")],
            &user,
        )
        .unwrap();

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &ManageVault(VaultMsg::ProcessMints),
            &[],
            &test_env.admin,
        )
        .unwrap();

    for (i, join_amounts) in JOINS.iter().enumerate() {
        execute_joins(
            &test_env,
            &modules,
            *join_amounts,
            &"uosmo".to_string(),
            1_000_000,
        );

        execute_joins(
            &test_env,
            &modules,
            JOINS[JOINS.len() - (i + 1)],
            &"uatom".to_string(),
            1_000_000,
        );

        cycle_positions(&test_env, &modules, true);
    }

    let bvt_balance = modules
        .bank
        .query_balance(&QueryBalanceRequest {
            address: user.address(),
            denom: format!("factory/{}/BVT", test_env.contract_addr),
        })
        .unwrap()
        .balance
        .unwrap()
        .amount
        .parse::<u128>()
        .unwrap();

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Burn {
                address: None,
                amount: None,
            }),
            &[coin(
                bvt_balance,
                format!("factory/{}/BVT", test_env.contract_addr),
            )],
            &user,
        )
        .unwrap();

    cycle_positions(&test_env, &modules, false);

    let uosmo_final_balance: Uint128 = modules
        .bank
        .query_balance(&QueryBalanceRequest {
            address: user.address(),
            denom: "uosmo".to_string(),
        })
        .unwrap()
        .balance
        .unwrap()
        .amount
        .parse::<u128>()
        .unwrap()
        .into();

    let uatom_final_balance: Uint128 = modules
        .bank
        .query_balance(&QueryBalanceRequest {
            address: user.address(),
            denom: "uatom".to_string(),
        })
        .unwrap()
        .balance
        .unwrap()
        .amount
        .parse::<u128>()
        .unwrap()
        .into();

    let final_dollar_value = Decimal::new(uosmo_final_balance).mul(get_price("uosmo"))
        + Decimal::new(uatom_final_balance).mul(get_price("uatom"));

    println!("final uosmo balance: {}", uosmo_final_balance);
    println!("final uatom balance: {}", uatom_final_balance);
    println!("final USD: {}", final_dollar_value);

    assert!(final_dollar_value >= intial_usd.mul(PRECISION.0.div(PRECISION.1)));
}

#[test]
fn test_queries() {
    let test_env = setup_contract(get_asset("uatom"));
    let modules = get_modules(&test_env);

    // let initial_contract_balance: TotalAssetsResponse = modules
    //     .wasm
    //     .query(&test_env.contract_addr, &TotalActiveAssets {})
    //     .unwrap();

    // let initial_contract_dollar_value = initial_contract_balance
    //     .asset0
    //     .amount
    //     .mul_floor(get_price("uosmo"))
    //     + initial_contract_balance
    //         .asset1
    //         .amount
    //         .mul_floor(get_price("uatom"));

    execute_joins(
        &test_env,
        &modules,
        JOINS[4],
        &"uosmo".to_string(),
        1_000_000,
    );
    execute_joins(
        &test_env,
        &modules,
        JOINS[5],
        &"uatom".to_string(),
        1_000_000,
    );

    cycle_positions(&test_env, &modules, false);

    let bvt_balance = modules
        .bank
        .query_balance(&QueryBalanceRequest {
            address: test_env.users[0].address(),
            denom: format!("factory/{}/BVT", test_env.contract_addr),
        })
        .unwrap()
        .balance
        .unwrap()
        .amount
        .parse::<u128>()
        .unwrap();

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Burn {
                address: None,
                amount: None,
            }),
            &[coin(
                bvt_balance,
                format!("factory/{}/BVT", test_env.contract_addr),
            )],
            &test_env.users[0],
        )
        .unwrap();

    let pendings_exits: Vec<AccountResponse> = modules
        .wasm
        .query(
            &test_env.contract_addr,
            &AccountStatus(AccountQuery::Burn(AccountQueryParams {
                address: None,
                start_after: None,
                limit: None,
            })),
        )
        .unwrap();

    assert_eq!(pendings_exits.len(), 1);

    // checks if user ratio converted to dollars accurately represents the funds they leave with
    // user ratio is determined by their share of the vault token supply
    let supply = modules
        .bank
        .query_total_supply(&QueryTotalSupplyRequest { pagination: None })
        .unwrap()
        .supply
        .iter()
        .find(|c| c.denom == format!("factory/{}/BVT", test_env.contract_addr))
        .unwrap()
        .amount
        .parse::<u128>()
        .unwrap();

    let user_ratio = Decimal::from_ratio(bvt_balance, supply);

    println!("user ratio: {}", user_ratio);

    let contract_balance: Vec<Coin> = modules
        .wasm
        .query(&test_env.contract_addr, &LockedAssets {})
        .unwrap();

    let contract_dollar_value: Decimal = Decimal::new(contract_balance[0].amount)
        .mul(get_price("uosmo"))
        + Decimal::new(contract_balance[1].amount).mul(get_price("uatom"));

    let uosmo_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uosmo".to_string())[0]);
    let uatom_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uatom".to_string())[0]);

    let dollar_balance = Decimal::new(uosmo_balance).mul(get_price("uosmo"))
        + Decimal::new(uatom_balance).mul(get_price("uatom"));

    cycle_positions(&test_env, &modules, false);

    let final_dollar_balance = Decimal::new(Uint128::new(
        user_balance_list(&test_env, &modules, "uosmo".to_string())[0],
    ))
    .mul(get_price("uosmo"))
        + Decimal::new(Uint128::new(
            user_balance_list(&test_env, &modules, "uatom".to_string())[0],
        ))
        .mul(get_price("uatom"));

    assert!(
        final_dollar_balance - dollar_balance
            >= user_ratio
                .mul(contract_dollar_value)
                .mul(PRECISION.0.div(PRECISION.1))
    );

    // check that TotalAssetsResponse return the value of assets we can use
    let new_contract_balance: Vec<Coin> = modules
        .wasm
        .query(&test_env.contract_addr, &LockedAssets {})
        .unwrap();

    let coins = Coins::try_from(new_contract_balance).unwrap();

    modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &ManagePosition(CreatePosition {
                lower_tick: -10000,
                upper_tick: 10000,
                tokens_provided: coins.into(),
                token_min_amount0: "1".to_string(),
                token_min_amount1: "1".to_string(),
                swap: None,
            }),
            &[],
            &test_env.admin,
        )
        .unwrap();
}

#[test]
fn test_cancel_mint() {
    let test_env = setup_contract(get_asset("uatom"));
    let modules = get_modules(&test_env);

    // simulate some join activity
    execute_joins(
        &test_env,
        &modules,
        JOINS[0],
        &"uosmo".to_string(),
        1_000_000,
    );

    cycle_positions(&test_env, &modules, false);

    // get user's pre deposit balance
    let start_uosmo_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uosmo".to_string())[0]);
    let start_uatom_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uatom".to_string())[0]);

    // let user[0] make a deposit for mint
    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Mint { min_out: None }),
            &[coin(53_000_000, "uatom"), coin(500_000_000, "uosmo")],
            &test_env.users[0],
        )
        .is_ok());

    // make sure user[0] can't join again
    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Mint { min_out: None }),
            &[coin(53_000_000, "uatom"), coin(500_000_000, "uosmo")],
            &test_env.users[0],
        )
        .is_err());

    // cancel user's mint
    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Cancel(CancelMsg::Mint),
            &[],
            &test_env.users[0],
        )
        .is_ok());

    // make sure post cancel balance is consistent, minus tx fees
    let uosmo_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uosmo".to_string())[0]);
    let uatom_balance =
        Uint128::new(user_balance_list(&test_env, &modules, "uatom".to_string())[0]);

    assert!(uatom_balance == start_uatom_balance);
    assert!(uosmo_balance == start_uosmo_balance - Uint128::from(FEE_AMOUNT * 3));
}

#[test]
fn test_cancel_burn() {
    let test_env = setup_contract(get_asset("uatom"));
    let modules = get_modules(&test_env);

    execute_joins(
        &test_env,
        &modules,
        JOINS[0],
        &"uosmo".to_string(),
        1_000_000,
    );

    cycle_positions(&test_env, &modules, false);

    let bvt_denom = format!("factory/{}/BVT", test_env.contract_addr);

    let bvt_balance = user_balance_list(&test_env, &modules, bvt_denom.clone())[0];

    // let user[0] make a deposit for burn then cancel it
    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Burn {
                address: None,
                amount: None
            }),
            &[coin(bvt_balance - 1_000_000, bvt_denom.clone())],
            &test_env.users[0],
        )
        .is_ok());

    // make sure they can't do another burn while the first is pending
    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Deposit(DepositMsg::Burn {
                address: None,
                amount: None
            }),
            &[coin(1_000_000, bvt_denom.clone())],
            &test_env.users[0],
        )
        .is_err());

    assert!(modules
        .wasm
        .execute(
            &test_env.contract_addr,
            &Cancel(CancelMsg::Burn),
            &[],
            &test_env.users[0],
        )
        .is_ok());

    // make sure user got all their bvt back
    let final_bvt_balance = user_balance_list(&test_env, &modules, bvt_denom)[0];

    assert!(bvt_balance == final_bvt_balance);
}
