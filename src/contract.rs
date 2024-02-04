use std::{collections::HashMap, str::FromStr};

use crate::{
    error::ContractError,
    msg::{ExecuteMsg, Frequency, InstantiateMsg, MigrateMsg, QueryMsg, Swap, TotalAssetsResponse},
    state::{
        Config, ACCOUNTS_PENDING_ACTIVATION, ADDRESSES_WAITING_FOR_EXIT, ASSETS_PENDING_ACTIVATION,
        CAP_REACHED, CONFIG, HALT_EXITS_AND_JOINS, LAST_EXIT, LAST_UPDATE, VAULT_RATIO,
        VAULT_TERMINATED,
    },
};
use cosmwasm_std::{
    attr, coin, entry_point, to_json_binary, Addr, Attribute, BankMsg, Binary, Coin, Coins,
    CosmosMsg, Decimal, Deps, DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult,
    Storage, Uint128, WasmMsg,
};
use cw2::{get_contract_version, set_contract_version};
use cw_ownable::{assert_owner, get_ownership, initialize_owner, update_ownership, Action};
use osmosis_std_modified::types::osmosis::{
    concentratedliquidity::v1beta1::{
        ConcentratedliquidityQuerier, FullPositionBreakdown, MsgAddToPosition,
        MsgCollectIncentives, MsgCreatePosition, Pool, UserPositionsResponse,
    },
    poolmanager::v1beta1::{MsgSplitRouteSwapExactAmountIn, PoolmanagerQuerier},
};
use osmosis_std_modified::types::{
    cosmos::base::v1beta1::Coin as CosmosCoin,
    osmosis::concentratedliquidity::v1beta1::MsgWithdrawPosition,
};
use pyth_sdk_cw::query_price_feed;

// version info for migration info
const CONTRACT_NAME: &str = env!("CARGO_PKG_NAME");
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const PYTH_TESTNET_CONTRACT_ADDRESS: &str =
    "osmo1hpdzqku55lmfmptpyj6wdlugqs5etr6teqf7r4yqjjrxjznjhtuqqu5kdh";
const PYTH_MAINNET_CONTRACT_ADDRESS: &str =
    "osmo13ge29x4e2s63a8ytz2px8gurtyznmue4a69n5275692v3qn3ks8q7cwck7";

// Used for the dead man switch. If admin didn't allow withdrawals for this amount of time, anyone can trigger the switch and all positions will be closed
// And people trying to leave will be able to leave
const MAX_NO_EXIT_PERIOD: u64 = 86400 * 14; // 14 days

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // validate and set the operator address
    deps.api.addr_validate(msg.operator.as_str())?;
    initialize_owner(deps.storage, deps.api, Some(msg.operator.as_str()))?;

    let pm_querier = PoolmanagerQuerier::new(&deps.querier);

    let pool_response = pm_querier.pool(msg.pool_id)?;

    let pool: Pool = match pool_response.pool {
        Some(pool) => {
            if pool
                .type_url
                .ne(&"/osmosis.concentratedliquidity.v1beta1.Pool".to_string())
            {
                return Err(ContractError::PoolIsNotCL {});
            }
            prost::Message::decode(pool.value.as_slice()).unwrap()
        }
        None => {
            return Err(ContractError::PoolNotFound {
                pool_id: msg.pool_id,
            });
        }
    };

    let pyth_contract_address = if msg.mainnet {
        PYTH_MAINNET_CONTRACT_ADDRESS
    } else {
        PYTH_TESTNET_CONTRACT_ADDRESS
    };

    // Validate that valid addresses were sent just in case
    if let Some(whitelisted_depositors) = msg.whitelisted_depositors.to_owned() {
        for address in whitelisted_depositors.iter() {
            deps.api.addr_validate(address.as_str())?;
        }
    };

    let config = Config {
        name: msg.name,
        description: msg.description,
        image: msg.image,
        pool_id: msg.pool_id,
        asset0: msg.asset0,
        asset1: msg.asset1,
        min_uptime: msg.min_uptime,
        dollar_cap: msg.dollar_cap,
        pyth_contract_address: Addr::unchecked(pyth_contract_address),
        price_expiry: msg.price_expiry,
        update_frequency: msg.update_frequency.to_owned(),
        commission: msg.commission,
        commission_receiver: msg.commission_receiver.unwrap_or(info.sender.to_owned()),
        whitelisted_depositors: msg.whitelisted_depositors,
    };

    // Check that the assets in the pool are the same assets we sent in the instantiate message
    verify_config(&config, pool)?;

    // Check that funds sent match with config
    verify_funds(&info, &config)?;

    // Check that funds sent are above minimum deposit
    verify_deposit_minimum(&info, &config)?;

    CONFIG.save(deps.storage, &config)?;
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
    )?;
    ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &vec![])?;

    // At the beginning, the instantiator owns 100% of the vault
    VAULT_RATIO.save(deps.storage, info.sender.to_owned(), &Decimal::one())?;
    // Set current block time/height as last update
    match msg.update_frequency {
        Frequency::Blocks(_) => {
            LAST_UPDATE.save(deps.storage, &env.block.height)?;
        }
        Frequency::Seconds(_) => {
            LAST_UPDATE.save(deps.storage, &env.block.time.seconds())?;
        }
    }

    CAP_REACHED.save(deps.storage, &false)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &false)?;
    VAULT_TERMINATED.save(deps.storage, &false)?;
    LAST_EXIT.save(deps.storage, &env.block.time.seconds())?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_instantiate")
        .add_attribute("contract_name", CONTRACT_NAME)
        .add_attribute("contract_version", CONTRACT_VERSION)
        .add_attribute("owner", info.sender))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::UpdateOwnership(action) => execute_update_ownership(deps, env, info, action),
        ExecuteMsg::ModifyConfig { config } => execute_modify_config(deps, env, info, *config),
        ExecuteMsg::Join {} => execute_join(deps, info),
        ExecuteMsg::Leave {} => execute_leave(deps, info),
        ExecuteMsg::CreatePosition {
            lower_tick,
            upper_tick,
            tokens_provided,
            token_min_amount0,
            token_min_amount1,
            swap,
        } => execute_create_position(
            deps,
            env,
            info,
            lower_tick,
            upper_tick,
            tokens_provided,
            token_min_amount0,
            token_min_amount1,
            swap,
        ),
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
            swap,
        } => execute_add_to_position(
            deps,
            env,
            info,
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
            swap,
        ),
        ExecuteMsg::WithdrawPosition {
            position_id,
            liquidity_amount,
        } => execute_withdraw_position(deps, env, info, position_id, liquidity_amount),
        ExecuteMsg::ProcessNewEntriesAndExits {} => {
            execute_process_new_entries_and_exits(deps, env, info)
        }
        ExecuteMsg::Halt {} => execute_halt(deps, info),
        ExecuteMsg::Resume {} => execute_resume(deps, info),
        ExecuteMsg::CloseVault {} => execute_close_vault(deps, info),
        ExecuteMsg::ForceExits {} => execute_force_exits(deps, env),
    }
}

fn execute_update_ownership(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    action: Action,
) -> Result<Response, ContractError> {
    let ownership = update_ownership(deps, &env.block, &info.sender, action)?;
    Ok(Response::new().add_attributes(ownership.into_attributes()))
}

fn execute_modify_config(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    new_config: Config,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    deps.api
        .addr_validate(new_config.pyth_contract_address.as_str())?;

    let old_config = CONFIG.load(deps.storage)?;

    if new_config.asset0 != old_config.asset0 || new_config.asset1 != old_config.asset1 {
        return Err(ContractError::CannotChangeAssets {});
    }

    if new_config.pool_id != old_config.pool_id {
        return Err(ContractError::CannotChangePoolId {});
    }

    // Validate that valid addresses were sent just in case
    if let Some(whitelisted_depositors) = new_config.whitelisted_depositors.to_owned() {
        for address in whitelisted_depositors.iter() {
            deps.api.addr_validate(address.as_str())?;
        }
    };

    CONFIG.save(deps.storage, &new_config)?;

    // If we modify config we are going to reset the timer for update (so we allow changing from blocks to seconds and viceversa)
    match new_config.update_frequency {
        Frequency::Blocks(_) => {
            LAST_UPDATE.save(deps.storage, &env.block.height)?;
        }
        Frequency::Seconds(_) => {
            LAST_UPDATE.save(deps.storage, &env.block.time.seconds())?;
        }
    }

    Ok(Response::new()
        .add_attribute("action", "banana_vault_modify_config")
        .add_attribute("new_pyth_address", new_config.pyth_contract_address))
}

fn execute_join(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    verify_funds(&info, &config)?;
    verify_deposit_minimum(&info, &config)?;

    // Check if vault is closed
    if VAULT_TERMINATED.load(deps.storage)? {
        return Err(ContractError::VaultClosed {});
    }

    // Check if vault is halted or cap has been reached
    if HALT_EXITS_AND_JOINS.load(deps.storage)? {
        return Err(ContractError::VaultHalted {});
    }

    // Check if vault cap has been reached
    if CAP_REACHED.load(deps.storage)? {
        // Check if user is whitelisted to exceed cap
        if !config
            .whitelisted_depositors
            .unwrap_or_default()
            .contains(&info.sender)
        {
            return Err(ContractError::CapReached {});
        }
    }

    // We queue up the assets for the next iteration
    let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    for asset in info.funds.iter() {
        if asset.denom == config.asset0.denom {
            assets_pending[0].amount += asset.amount;
        } else if asset.denom == config.asset1.denom {
            assets_pending[1].amount += asset.amount;
        } else {
            return Err(ContractError::InvalidFunds {});
        }
    }

    ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

    // Check if user added to the current pending amount or if it's the first time he added

    match ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, info.sender.to_owned())? {
        Some(mut funds) => {
            for fund in info.funds.iter() {
                if fund.denom == config.asset0.denom {
                    funds[0].amount += fund.amount;
                } else if fund.denom == config.asset1.denom {
                    funds[1].amount += fund.amount;
                }
            }
            ACCOUNTS_PENDING_ACTIVATION.save(deps.storage, info.sender.to_owned(), &funds)?;
        }
        None => {
            let mut amounts_to_add = vec![
                coin(0, config.asset0.denom.to_owned()),
                coin(0, config.asset1.denom.to_owned()),
            ];

            for fund in info.funds.iter() {
                if fund.denom == config.asset0.denom {
                    amounts_to_add[0].amount += fund.amount;
                } else if fund.denom == config.asset1.denom {
                    amounts_to_add[1].amount += fund.amount;
                }
            }
            ACCOUNTS_PENDING_ACTIVATION.save(
                deps.storage,
                info.sender.to_owned(),
                &amounts_to_add,
            )?;
        }
    }

    Ok(Response::new()
        .add_attribute("action", "banana_vault_join")
        .add_attribute("address", info.sender))
}

fn execute_leave(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    // If user wants to leave, we return any pending joining assets and add him in the list for leaving the vault if he has active assets in it
    let mut response = Response::new();

    // Check if vault is not halted for security reasons
    if HALT_EXITS_AND_JOINS.load(deps.storage)? {
        return Err(ContractError::VaultHalted {});
    }

    if let Some(mut funds) =
        ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, info.sender.to_owned())?
    {
        // We return the pending joining assets
        let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
        assets_pending[0].amount -= funds[0].amount;
        assets_pending[1].amount -= funds[1].amount;
        ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

        ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, info.sender.to_owned());

        // Remove empty amounts to avoid sending empty funds in bank msg
        funds.retain(|f| f.amount.ne(&Uint128::zero()));

        let send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: funds,
        };

        response = response.add_message(send_msg);
    }

    if VAULT_RATIO.has(deps.storage, info.sender.to_owned()) {
        // We add the user to the list of addresses waiting to leave the vault
        let mut addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;
        if !addresses_waiting_for_exit.contains(&info.sender) {
            addresses_waiting_for_exit.push(info.sender.to_owned());
            ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &addresses_waiting_for_exit)?;
        }
    }

    Ok(response
        .add_attribute("action", "banana_vault_leave")
        .add_attribute("sender", info.sender))
}

#[allow(clippy::too_many_arguments)]
fn execute_create_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    lower_tick: i64,
    upper_tick: i64,
    tokens_provided: Vec<Coin>,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let mut messages = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset0.denom.to_owned(),
    )?;
    let mut balance_asset1 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset1.denom.to_owned(),
    )?;

    // execute swap if provided
    if let Some(swap) = swap {
        let (message, attribute) =
            prepare_swap(&deps, env.contract.address.to_string(), swap.clone())?;
        messages.push(message);
        attributes.push(attribute);

        (balance_asset0, balance_asset1) =
            parse_swap_result(&mut balance_asset0, &mut balance_asset1, swap)?;
    }

    verify_availability_of_funds(
        deps.storage,
        tokens_provided.clone(),
        balance_asset0,
        balance_asset1,
    )?;

    messages.push(
        MsgCreatePosition {
            pool_id: config.pool_id,
            sender: env.contract.address.to_string(),
            lower_tick,
            upper_tick,
            tokens_provided: tokens_provided
                .iter()
                .map(|coin| CosmosCoin {
                    denom: coin.denom.to_string(),
                    amount: coin.amount.to_string(),
                })
                .collect(),
            token_min_amount0,
            token_min_amount1,
        }
        .into(),
    );

    attributes.push(attr("action", "banana_vault_create_position"));

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

#[allow(clippy::too_many_arguments)]
fn execute_add_to_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: u64,
    amount0: String,
    amount1: String,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let contract_address = env.contract.address;
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps
        .querier
        .query_balance(contract_address.clone(), config.asset0.denom.to_owned())?;
    let mut balance_asset1 = deps
        .querier
        .query_balance(contract_address.clone(), config.asset1.denom.to_owned())?;

    // Collect rewards instead of letting them be claimed when adding to position
    let (reward0, reward1, reward_messages, reward_attributes) =
        collect_rewards(&deps, contract_address.to_string(), position_id)?;

    balance_asset0.amount += reward0;
    balance_asset1.amount += reward1;
    messages.extend(reward_messages);
    attributes.extend(reward_attributes);

    // execute swap if provided
    if let Some(swap) = swap {
        let (message, attribute) = prepare_swap(&deps, contract_address.to_string(), swap.clone())?;
        messages.push(message);
        attributes.push(attribute);

        (balance_asset0, balance_asset1) =
            parse_swap_result(&mut balance_asset0, &mut balance_asset1, swap)?;
    }

    let tokens_provided = vec![
        coin(amount0.parse::<u128>()?, config.asset0.denom.to_owned()),
        coin(amount1.parse::<u128>()?, config.asset1.denom.to_owned()),
    ];

    verify_availability_of_funds(
        deps.storage,
        tokens_provided.clone(),
        balance_asset0,
        balance_asset1,
    )?;

    messages.push(
        MsgAddToPosition {
            position_id,
            sender: contract_address.to_string(),
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
        }
        .into(),
    );

    attributes.push(attr("action", "banana_vault_add_to_position"));

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

fn execute_withdraw_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: u64,
    liquidity_amount: String,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;

    let config = CONFIG.load(deps.storage)?;

    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);

    // if uptime is set, check if enough time has passed to withdraw
    if let Some(uptime) = config.min_uptime {
        let join_time: u64 = match cl_querier.position_by_id(position_id)?.position {
            Some(position) => position.position.unwrap().join_time.unwrap().seconds as u64,
            None => {
                return Err(ContractError::NoPositionsOpen {});
            }
        };

        if env.block.time.seconds() - join_time < uptime {
            return Err(ContractError::MinUptime());
        }
    }

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    // Collect all rewards first to claim commission and distribute the rest
    let (_, _, reward_messages, reward_attributes) =
        collect_rewards(&deps, env.contract.address.to_string(), position_id)?;
    messages.extend(reward_messages);
    attributes.extend(reward_attributes);

    let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
        position_id,
        sender: env.contract.address.to_string(),
        liquidity_amount,
    }
    .into();

    messages.push(msg_withdraw_position);
    attributes.push(attr("action", "banana_vault_withdraw_position"));

    let last_update = LAST_UPDATE.load(deps.storage)?;

    // We will only execute this message if it's time for update (it will execute after the withdraw and check if there are any positions open)
    let update_users_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: env.contract.address.to_string(),
        msg: to_json_binary(&ExecuteMsg::ProcessNewEntriesAndExits {})?,
        funds: vec![],
    });

    // We can only update if it's time for update and if vault is not halted
    if !HALT_EXITS_AND_JOINS.load(deps.storage)? {
        match config.update_frequency {
            Frequency::Blocks(blocks) => {
                if env.block.height >= last_update + blocks {
                    messages.push(update_users_msg)
                }
            }
            Frequency::Seconds(seconds) => {
                if env.block.time.seconds() >= last_update + seconds {
                    messages.push(update_users_msg)
                }
            }
        }
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

// collect_rewards should be called upon any position change
fn collect_rewards(
    deps: &DepsMut,
    sender: String,
    position_id: u64,
) -> Result<(Uint128, Uint128, Vec<CosmosMsg>, Vec<Attribute>), ContractError> {
    let config = CONFIG.load(deps.storage)?;

    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);

    let position: FullPositionBreakdown = match cl_querier.position_by_id(position_id)?.position {
        Some(position) => position,
        None => {
            return Err(ContractError::NoPositionsOpen {});
        }
    };

    let mut reward_coins: Coins = Coins::default();

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    if !position.claimable_incentives.is_empty() {
        for incentive in &position.claimable_incentives {
            reward_coins.add(coin(
                Uint128::from_str(&incentive.amount)?.u128(),
                incentive.denom.to_owned(),
            ))?;
        }
        messages.push(
            MsgCollectIncentives {
                position_ids: vec![position_id],
                sender: sender.clone(),
            }
            .into(),
        );
    }

    if !position.claimable_spread_rewards.is_empty() {
        for incentive in &position.claimable_spread_rewards {
            reward_coins.add(coin(
                Uint128::from_str(&incentive.amount)?.u128(),
                incentive.denom.to_owned(),
            ))?;
        }
        messages.push(
            MsgCollectIncentives {
                position_ids: vec![position_id],
                sender,
            }
            .into(),
        );
    }

    if !reward_coins.is_empty() {
        attributes.push(attr("action", "banana_vault_collect_rewards"));
    }

    let mut amount0 = Uint128::zero();
    let mut amount1 = Uint128::zero();
    let mut commission_coins: Coins = Coins::default();

    if let Some(commission) = config.commission {
        for reward_coin in &reward_coins {
            let commission_coin = coin(
                reward_coin.amount.mul_floor(commission).u128(),
                reward_coin.denom.to_owned(),
            );
            commission_coins.add(commission_coin.clone())?;
            if reward_coin.denom == config.asset0.denom {
                amount0 = reward_coin.amount.checked_sub(commission_coin.amount)?;
            } else if reward_coin.denom == config.asset1.denom {
                amount1 = reward_coin.amount.checked_sub(commission_coin.amount)?;
            }
        }
    }

    if !commission_coins.is_empty() {
        let send_msg = BankMsg::Send {
            to_address: config.commission_receiver.to_string(),
            amount: commission_coins.into(),
        };
        attributes.push(attr("action", "banana_vault_collect_commission"));
        messages.push(send_msg.into());
    }

    Ok((amount0, amount1, messages, attributes))
}

// This can only be done by contract internally
fn execute_process_new_entries_and_exits(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    if info.sender != env.contract.address {
        return Err(ContractError::Unauthorized {});
    }

    let config = CONFIG.load(deps.storage)?;

    // We will only do this is no current positions are open
    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    let user_positions_response: UserPositionsResponse =
        cl_querier.user_positions(env.contract.address.to_string(), config.pool_id, None)?;

    let mut messages = vec![];
    let mut response = Response::new();
    if user_positions_response.positions.is_empty() {
        // These are all the assets the vault has that are not pending to join
        let balance_asset0 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset0.denom.to_owned(),
        )?;
        let balance_asset1 = deps
            .querier
            .query_balance(env.contract.address, config.asset1.denom.to_owned())?;

        let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

        let available_in_vault_asset0 = coin(
            balance_asset0
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset0.denom.to_owned(),
        );

        let available_in_vault_asset1 = coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        );

        let addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;

        // We are going to process all exits first
        for exit_address in addresses_waiting_for_exit.iter() {
            let ratio = VAULT_RATIO.load(deps.storage, exit_address.to_owned())?;
            let amount_to_send_asset0 = available_in_vault_asset0.amount.mul_floor(ratio);
            let amount_to_send_asset1 = available_in_vault_asset1.amount.mul_floor(ratio);

            let mut amounts_send_msg = vec![];
            if amount_to_send_asset0.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset0.u128(),
                    config.asset0.denom.to_owned(),
                ));
            }

            if amount_to_send_asset1.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset1.u128(),
                    config.asset1.denom.to_owned(),
                ));
            }

            let send_msg = BankMsg::Send {
                to_address: exit_address.to_string(),
                amount: amounts_send_msg,
            };

            messages.push(send_msg);
            // He doesn't own any vault assets anymore
            VAULT_RATIO.remove(deps.storage, exit_address.to_owned());
        }

        ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &vec![])?;
        // After processing all exits, we are going to see how much the previous vault participants and new participants own (in dollars) to recalculate vault ratios
        // Get prices in dollars for each asset. for safety we check that the price has been updated within the configured window
        let current_time = env.block.time.seconds() as i64;

        let current_price_asset0 = match query_price_feed(
            &deps.querier,
            config.pyth_contract_address.clone(),
            config.asset0.price_identifier,
        )?
        .price_feed
        .get_price_no_older_than(current_time, config.price_expiry)
        {
            // If there is a price available, calculate the current price as a Decimal
            Some(price) => {
                Decimal::from_ratio(price.price as u128, 10_u64.pow(config.asset0.decimals))
            }

            // Return an error if no price is available within the acceptable age
            None => {
                return Err(ContractError::StalePrice {
                    seconds: config.price_expiry,
                })
            }
        };

        let current_price_asset1 = match query_price_feed(
            &deps.querier,
            config.pyth_contract_address,
            config.asset1.price_identifier,
        )?
        .price_feed
        .get_price_no_older_than(current_time, config.price_expiry)
        {
            // If there is a price available, calculate the current price as a Decimal
            Some(price) => {
                Decimal::from_ratio(price.price as u128, 10_u64.pow(config.asset1.decimals))
            }

            // Return an error if no price is available within the acceptable age
            None => {
                return Err(ContractError::StalePrice {
                    seconds: config.price_expiry,
                })
            }
        };

        // Now that we have current prices of both assets we are going to see how much each address owns in dollars and store it to recalculate vault ratios
        let mut total_dollars_in_vault = Decimal::zero();
        let mut address_dollars_map: HashMap<Addr, Decimal> = HashMap::new();

        let ratios: Vec<(Addr, Decimal)> = VAULT_RATIO
            .range(deps.storage, None, None, Order::Ascending)
            .filter_map(|v| v.ok())
            .collect();

        for ratio in ratios.iter() {
            let mut amount_assets0 = available_in_vault_asset0.amount.mul_floor(ratio.1);
            let mut amount_assets1 = available_in_vault_asset1.amount.mul_floor(ratio.1);

            // If for some reason this address is actually waiting to join with more assets, we will add this here too
            if let Some(funds) =
                ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, ratio.0.to_owned())?
            {
                amount_assets0 += funds[0].amount;
                amount_assets1 += funds[1].amount;
                // Remove it from here to avoid processing it again later
                ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, ratio.0.to_owned())
            }

            let dollars_asset0 = current_price_asset0.checked_mul(Decimal::new(amount_assets0))?;
            let dollars_asset1 = current_price_asset1.checked_mul(Decimal::new(amount_assets1))?;

            let total_amount_dollars_for_address = dollars_asset0.checked_add(dollars_asset1)?;

            total_dollars_in_vault =
                total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

            address_dollars_map.insert(ratio.0.to_owned(), total_amount_dollars_for_address);
        }

        // Now we are going to process all the new entries
        let new_entries: Vec<(Addr, Vec<Coin>)> = ACCOUNTS_PENDING_ACTIVATION
            .range(deps.storage, None, None, Order::Ascending)
            .filter_map(|v| v.ok())
            .collect();

        for new_entry in new_entries.iter() {
            let dollars_asset0 =
                current_price_asset0.checked_mul(Decimal::new(new_entry.1[0].amount))?;
            let dollars_asset1 =
                current_price_asset1.checked_mul(Decimal::new(new_entry.1[1].amount))?;

            let total_amount_dollars_for_address = dollars_asset0.checked_add(dollars_asset1)?;

            total_dollars_in_vault =
                total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

            address_dollars_map.insert(new_entry.0.to_owned(), total_amount_dollars_for_address);
        }

        // Now we have all addresses with their corresponding amount in dollars and the total amount of dollars in the vault, let's recalculate ratios

        // Clear the map
        VAULT_RATIO.clear(deps.storage);

        // Recalculate ratios
        for each_address in address_dollars_map.iter() {
            let ratio = each_address.1.checked_div(total_dollars_in_vault)?;
            VAULT_RATIO.save(deps.storage, each_address.0.to_owned(), &ratio)?;
        }

        // Clean all pending activation addresses and amount
        ACCOUNTS_PENDING_ACTIVATION.clear(deps.storage);
        ASSETS_PENDING_ACTIVATION.save(
            deps.storage,
            &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
        )?;

        // Save last update to current block time/height
        match config.update_frequency {
            Frequency::Blocks(_) => {
                LAST_UPDATE.save(deps.storage, &env.block.height)?;
            }
            Frequency::Seconds(_) => {
                LAST_UPDATE.save(deps.storage, &env.block.time.seconds())?;
            }
        }

        LAST_EXIT.save(deps.storage, &env.block.time.seconds())?;

        // Check that we are not over the vault cap, if that's the case, we will flag it to halt joins until under cap again
        if let Some(dollar_cap) = config.dollar_cap {
            if total_dollars_in_vault >= Decimal::new(dollar_cap) {
                CAP_REACHED.save(deps.storage, &true)?;
            } else {
                CAP_REACHED.save(deps.storage, &false)?;
            }
        }

        // All bank messages that need to be sent
        response = response.add_messages(messages)
    }

    Ok(response.add_attribute("action", "banana_vault_process_new_entries_and_exits"))
}

fn prepare_swap(
    deps: &DepsMut,
    sender: String,
    swap: Swap,
) -> Result<(CosmosMsg, Attribute), ContractError> {
    let config = CONFIG.load(deps.storage)?;
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    let mut token_in_amount = 0;
    for route in &swap.routes {
        token_in_amount += route.token_in_amount.parse::<u128>()?;
    }

    // We are not allowed to use more than what we have currently available in the vault
    if swap.token_in_denom == config.asset0.denom {
        let balance_asset0 = deps
            .querier
            .query_balance(sender.clone(), config.asset0.denom.to_owned())?;

        let available_to_swap_asset0 = coin(
            balance_asset0
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset0.denom.to_owned(),
        );

        if token_in_amount > available_to_swap_asset0.amount.u128() {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    if swap.token_in_denom == config.asset1.denom {
        let balance_asset1 = deps
            .querier
            .query_balance(sender.clone(), config.asset1.denom.to_owned())?;

        let available_to_swap_asset1 = coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        );

        if token_in_amount > available_to_swap_asset1.amount.u128() {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    let msg_split_route_swap_exact_amount_in: CosmosMsg = MsgSplitRouteSwapExactAmountIn {
        sender,
        routes: swap.routes,
        token_in_denom: swap.token_in_denom,
        token_out_min_amount: swap.token_out_min_amount,
    }
    .into();

    Ok((
        msg_split_route_swap_exact_amount_in,
        attr("action", "banana_vault_swap"),
    ))
}

fn execute_halt(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &true)?;
    Ok(Response::new().add_attribute("action", "banana_vault_halt"))
}

fn execute_resume(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &false)?;
    Ok(Response::new().add_attribute("action", "banana_vault_resume"))
}

fn execute_close_vault(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let mut messages = vec![];

    // We get all addresses that are waiting to join and send the funds back

    let addresses_pending_activation: Vec<(Addr, Vec<Coin>)> = ACCOUNTS_PENDING_ACTIVATION
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|v| v.ok())
        .collect();

    for each_address in addresses_pending_activation.iter() {
        let mut funds = each_address.1.to_owned();

        // Remove empty amounts to avoid sending empty funds in bank msg
        funds.retain(|f| f.amount.ne(&Uint128::zero()));

        let send_msg = BankMsg::Send {
            to_address: each_address.0.to_string(),
            amount: funds,
        };

        messages.push(send_msg);
    }

    // We get all addresses that are waiting to exit and add all the ones that are in the vault and are not waiting for exit
    let mut addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;
    let ratios: Vec<(Addr, Decimal)> = VAULT_RATIO
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|v| v.ok())
        .collect();

    for ratio in ratios.iter() {
        if !addresses_waiting_for_exit.contains(&ratio.0) {
            addresses_waiting_for_exit.push(ratio.0.to_owned());
        }
    }

    // Save all in state
    // No one else is waiting to join (we sent all funds back)
    // No funds are waiting to join
    // Everyone is added to exit list
    // Vault is terminated (no one can join anymore)

    ACCOUNTS_PENDING_ACTIVATION.clear(deps.storage);
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
    )?;
    ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &addresses_waiting_for_exit)?;
    VAULT_TERMINATED.save(deps.storage, &true)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "banana_vault_terminate"))
}

fn execute_force_exits(deps: DepsMut, env: Env) -> Result<Response, ContractError> {
    let last_exit = LAST_EXIT.load(deps.storage)?;

    if last_exit.checked_add(MAX_NO_EXIT_PERIOD).unwrap() < env.block.time.seconds() {
        return Err(ContractError::CantForceExitsYet {
            seconds: env.block.time.seconds() - last_exit.checked_add(MAX_NO_EXIT_PERIOD).unwrap(),
        });
    }

    let mut messages = vec![];
    let config = CONFIG.load(deps.storage)?;
    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    let user_positions_response: UserPositionsResponse =
        cl_querier.user_positions(env.contract.address.to_string(), config.pool_id, None)?;

    for position in user_positions_response.positions.iter() {
        let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
            position_id: position.position.as_ref().unwrap().position_id,
            sender: env.contract.address.to_string(),
            liquidity_amount: position.position.as_ref().unwrap().liquidity.to_owned(),
        }
        .into();

        messages.push(msg_withdraw_position);
    }

    let update_users_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: env.contract.address.to_string(),
        msg: to_json_binary(&ExecuteMsg::ProcessNewEntriesAndExits {})?,
        funds: vec![],
    });

    messages.push(update_users_msg);

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "banana_vault_force_exits"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::Ownership {} => to_json_binary(&get_ownership(deps.storage)?),
        QueryMsg::TotalActiveAssets {} => to_json_binary(&query_total_active_assets(deps, env)?),
        QueryMsg::TotalPendingAssets {} => to_json_binary(&query_total_pending_assets(deps)?),
        QueryMsg::PendingJoin { address } => to_json_binary(&query_pending_join(deps, address)?),
        QueryMsg::VaultRatio { address } => to_json_binary(&query_vault_ratio(deps, address)?),
    }
}

fn query_config(deps: Deps) -> StdResult<Config> {
    let config = CONFIG.load(deps.storage)?;
    Ok(config)
}

fn query_total_active_assets(deps: Deps, env: Env) -> StdResult<TotalAssetsResponse> {
    let config = CONFIG.load(deps.storage)?;
    let assets_pending = ASSETS_PENDING_ACTIVATION
        .may_load(deps.storage)?
        .unwrap_or(vec![
            coin(0, CONFIG.load(deps.storage)?.asset0.denom),
            coin(0, CONFIG.load(deps.storage)?.asset1.denom),
        ]);

    let address = env.contract.address.to_string();

    let mut balance_asset0 = deps
        .querier
        .query_balance(address.clone(), config.asset0.denom.to_owned())?;
    let mut balance_asset1 = deps
        .querier
        .query_balance(env.contract.address, config.asset1.denom.to_owned())?;

    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    for position in &cl_querier
        .user_positions(address, config.pool_id, None)?
        .positions
    {
        balance_asset0.amount = balance_asset0.amount.checked_add(
            position
                .asset0
                .clone()
                .unwrap_or_default()
                .amount
                .parse::<Uint128>()?,
        )?;
        balance_asset1.amount = balance_asset1.amount.checked_add(
            position
                .asset1
                .clone()
                .unwrap_or_default()
                .amount
                .parse::<Uint128>()?,
        )?;

        let mut reward_coins: Coins = Coins::default();

        if !position.claimable_incentives.is_empty() {
            for incentive in &position.claimable_incentives {
                reward_coins.add(coin(
                    Uint128::from_str(&incentive.amount)?.u128(),
                    incentive.denom.to_owned(),
                ))?;
            }
        }

        if !position.claimable_spread_rewards.is_empty() {
            for incentive in &position.claimable_spread_rewards {
                reward_coins.add(coin(
                    Uint128::from_str(&incentive.amount)?.u128(),
                    incentive.denom.to_owned(),
                ))?;
            }
        }

        if let Some(commission) = config.commission {
            for reward_coin in &reward_coins {
                if reward_coin.denom == config.asset0.denom {
                    balance_asset0.amount += reward_coin
                        .amount
                        .checked_sub(reward_coin.amount.mul_floor(commission))?;
                } else if reward_coin.denom == config.asset1.denom {
                    balance_asset1.amount += reward_coin
                        .amount
                        .checked_sub(reward_coin.amount.mul_floor(commission))?;
                }
            }
        }
    }

    Ok(TotalAssetsResponse {
        asset0: coin(
            balance_asset0
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset0.denom,
        ),
        asset1: coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset1.denom,
        ),
    })
}

fn query_total_pending_assets(deps: Deps) -> StdResult<TotalAssetsResponse> {
    let config = CONFIG.load(deps.storage)?;
    let asset0_denom = config.asset0.denom;
    let asset1_denom = config.asset1.denom;
    let mut asset0 = coin(0, asset0_denom.to_owned());
    let mut asset1 = coin(0, asset1_denom.to_owned());

    for asset in ASSETS_PENDING_ACTIVATION.load(deps.storage)? {
        if asset.denom == asset0_denom {
            asset0.amount += asset.amount;
        } else if asset.denom == asset1_denom {
            asset1.amount += asset.amount;
        }
    }

    Ok(TotalAssetsResponse { asset0, asset1 })
}

fn query_pending_join(deps: Deps, address: Addr) -> StdResult<Vec<Coin>> {
    let assets = ACCOUNTS_PENDING_ACTIVATION
        .may_load(deps.storage, address)?
        .unwrap_or_default();
    Ok(assets)
}

fn query_vault_ratio(deps: Deps, address: Addr) -> StdResult<Decimal> {
    let ratio = VAULT_RATIO
        .may_load(deps.storage, address)?
        .unwrap_or_default();
    Ok(ratio)
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    let version = get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    Ok(Response::default())
}

// Helpers
fn verify_config(config: &Config, pool: Pool) -> Result<(), ContractError> {
    if config.asset0.denom != pool.token0 && config.asset0.denom != pool.token1 {
        return Err(ContractError::InvalidConfigAsset {});
    }

    if config.asset1.denom != pool.token0 && config.asset1.denom != pool.token1 {
        return Err(ContractError::InvalidConfigAsset {});
    }

    Ok(())
}

fn verify_funds(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    if info.funds.is_empty() {
        return Err(ContractError::NoFunds {});
    }

    if info.funds.len() == 2 && info.funds[0].denom == info.funds[1].denom {
        return Err(ContractError::InvalidFunds {});
    }

    if info.funds.len() > 2 {
        return Err(ContractError::InvalidFunds {});
    }

    for fund in info.funds.iter() {
        if fund.denom != config.asset0.denom && fund.denom != config.asset1.denom {
            return Err(ContractError::InvalidFunds {});
        }
    }

    Ok(())
}

fn verify_deposit_minimum(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    for token_provided in info.funds.iter() {
        if token_provided.denom == config.asset0.denom
            && token_provided.amount.lt(&config.asset0.min_deposit)
        {
            return Err(ContractError::DepositBelowMinimum {
                denom: config.asset0.denom.clone(),
            });
        }
        if token_provided.denom == config.asset1.denom
            && token_provided.amount.lt(&config.asset1.min_deposit)
        {
            return Err(ContractError::DepositBelowMinimum {
                denom: config.asset1.denom.clone(),
            });
        }
    }

    Ok(())
}

fn verify_availability_of_funds(
    storage: &mut dyn Storage,
    tokens_provided: Vec<Coin>,
    balance_asset0: Coin,
    balance_asset1: Coin,
) -> Result<(), ContractError> {
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(storage)?;
    let config = CONFIG.load(storage)?;

    let available_to_add_asset0 = coin(
        balance_asset0
            .amount
            .checked_sub(assets_pending[0].amount)?
            .u128(),
        config.asset0.denom.to_owned(),
    );

    let available_to_add_asset1 = coin(
        balance_asset1
            .amount
            .checked_sub(assets_pending[1].amount)?
            .u128(),
        config.asset1.denom.to_owned(),
    );

    for token_provided in tokens_provided.iter() {
        if token_provided.denom == config.asset0.denom
            && token_provided.amount > available_to_add_asset0.amount
        {
            return Err(ContractError::CannotAddMoreThenAvailableForAsset {
                asset: config.asset0.denom,
            });
        }
        if token_provided.denom == config.asset1.denom
            && token_provided.amount > available_to_add_asset1.amount
        {
            return Err(ContractError::CannotAddMoreThenAvailableForAsset {
                asset: config.asset1.denom,
            });
        }
    }

    Ok(())
}

fn parse_swap_result(token0: &mut Coin, token1: &mut Coin, swap: Swap) -> StdResult<(Coin, Coin)> {
    // every route will end in the same denom. we get the final denom from the first route and add the min amount out to the input assets
    let last_pool = swap
        .routes
        .get(0)
        .and_then(|route| route.pools.last())
        .unwrap();

    let token_out_min_amount = Uint128::from_str(&swap.token_out_min_amount)?;
    if last_pool.token_out_denom == token0.denom {
        token0.amount += token_out_min_amount;
    } else if last_pool.token_out_denom == token1.denom {
        token1.amount += token_out_min_amount;
    }

    // iterate thru the routes and subtract the amounts in from input coins
    for route in &swap.routes {
        let token_in_amount = Uint128::from_str(&route.token_in_amount)?;
        if swap.token_in_denom == token0.denom {
            token0.amount = token0.amount.checked_sub(token_in_amount)?;
        } else if swap.token_in_denom == token1.denom {
            token1.amount = token1.amount.checked_sub(token_in_amount)?;
        }
    }

    Ok((token0.clone(), token1.clone()))
}
