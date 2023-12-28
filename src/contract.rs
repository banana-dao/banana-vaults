use std::collections::HashMap;

use crate::{
    error::ContractError,
    msg::{ActiveVaultAssetsResponse, ExecuteMsg, Frequency, InstantiateMsg, MigrateMsg, QueryMsg},
    state::{
        Config, ACCOUNTS_PENDING_ACTIVATION, ADDRESSES_WAITING_FOR_EXIT, ASSETS_PENDING_ACTIVATION,
        CONFIG, LAST_UPDATE, VAULT_RATIO,
    },
};
use cosmwasm_std::{
    coin, entry_point, to_json_binary, Addr, BankMsg, Binary, Coin, CosmosMsg, Decimal, Deps,
    DepsMut, Env, MessageInfo, Order, Response, StdError, StdResult, Storage, Uint128, WasmMsg,
};
use cw2::{get_contract_version, set_contract_version};
use cw_ownable::{assert_owner, get_ownership, initialize_owner, update_ownership, Action};
use osmosis_std::types::osmosis::{
    concentratedliquidity::v1beta1::{
        ConcentratedliquidityQuerier, MsgAddToPosition, MsgCreatePosition, Pool,
    },
    gamm::v1beta1::MsgSwapExactAmountIn,
    poolmanager::v1beta1::{
        MsgSplitRouteSwapExactAmountIn, PoolmanagerQuerier, SwapAmountInRoute,
        SwapAmountInSplitRoute,
    },
};
use osmosis_std::types::{
    cosmos::base::v1beta1::Coin as CosmosCoin,
    osmosis::concentratedliquidity::v1beta1::MsgWithdrawPosition,
};
use pyth_sdk_cw::{query_price_feed, PriceFeedResponse};

// version info for migration info
const CONTRACT_NAME: &str = env!("CARGO_PKG_NAME");
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const PYTH_TESTNET_CONTRACT_ADDRESS: &str =
    "osmo1hpdzqku55lmfmptpyj6wdlugqs5etr6teqf7r4yqjjrxjznjhtuqqu5kdh";
const PYTH_MAINNET_CONTRACT_ADDRESS: &str =
    "osmo13ge29x4e2s63a8ytz2px8gurtyznmue4a69n5275692v3qn3ks8q7cwck7";

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    initialize_owner(deps.storage, deps.api, Some(info.sender.as_str()))?;

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

    let config = Config {
        pool_id: msg.pool_id,
        asset1: msg.asset1,
        asset2: msg.asset2,
        pyth_contract_address: Addr::unchecked(pyth_contract_address),
        update_frequency: msg.update_frequency.to_owned(),
        exit_commission: msg.exit_commission,
    };

    // Check that the assets in the pool are the same assets we sent in the instantiate message
    verify_config(&config, pool)?;

    // Check that funds sent match with config
    verify_funds(&info, &config)?;

    CONFIG.save(deps.storage, &config)?;
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset1.denom), coin(0, config.asset2.denom)],
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

    Ok(Response::new()
        .add_attribute("action", "instantiate_banana_vault")
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
        ExecuteMsg::ModifyConfig { config } => execute_modify_config(deps, env, info, config),
        ExecuteMsg::Join {} => execute_join(deps, info),
        ExecuteMsg::Leave {} => execute_leave(deps, info),
        ExecuteMsg::CreatePosition {
            lower_tick,
            upper_tick,
            tokens_provided,
            token_min_amount0,
            token_min_amount1,
        } => execute_create_position(
            deps,
            env,
            info,
            lower_tick,
            upper_tick,
            tokens_provided,
            token_min_amount0,
            token_min_amount1,
        ),
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
        } => execute_add_to_position(
            deps,
            env,
            info,
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
        ),
        ExecuteMsg::WithdrawPosition {
            position_id,
            liquidity_amount,
        } => execute_withdraw_position(deps, env, info, position_id, liquidity_amount),
        ExecuteMsg::SwapExactAmountIn {
            routes,
            token_in,
            token_out_min_amount,
        } => execute_swap_exact_amount_in(deps, env, info, routes, token_in, token_out_min_amount),
        ExecuteMsg::SplitRouteSwapExactAmountIn {
            routes,
            token_in_denom,
            token_out_min_amount,
        } => execute_split_route_swap_exact_amount_in(
            deps,
            env,
            info,
            routes,
            token_in_denom,
            token_out_min_amount,
        ),
        ExecuteMsg::ProcessNewEntriesAndExits {} => {
            execute_process_new_entries_and_exits(deps, env, info)
        }
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

    if new_config.asset1 != old_config.asset1 || new_config.asset2 != old_config.asset2 {
        return Err(ContractError::CannotChangeAssets {});
    }

    if new_config.pool_id != old_config.pool_id {
        return Err(ContractError::CannotChangePoolId {});
    }

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
        .add_attribute("action", "modify_config")
        .add_attribute("new_pyth_address", new_config.pyth_contract_address))
}

fn execute_join(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    verify_funds(&info, &config)?;

    // We queue up the assets for the next iteration
    let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
    assets_pending[0].amount += info.funds[0].amount;
    assets_pending[1].amount += info.funds[1].amount;
    ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

    // Check if user added to the current pending amount or if it's the first time he added
    match ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, info.sender.to_owned())? {
        Some(mut funds) => {
            funds[0].amount += info.funds[0].amount;
            funds[1].amount += info.funds[1].amount;
            ACCOUNTS_PENDING_ACTIVATION.save(deps.storage, info.sender.to_owned(), &funds)?;
        }
        None => {
            ACCOUNTS_PENDING_ACTIVATION.save(
                deps.storage,
                info.sender.to_owned(),
                &vec![info.funds[0].clone(), info.funds[1].clone()],
            )?;
        }
    }

    Ok(Response::new()
        .add_attribute("action", "join_banana_vault")
        .add_attribute("address", info.sender)
        .add_attribute("amount_asset_1", info.funds[0].amount)
        .add_attribute("amount_asset_2", info.funds[1].amount))
}

fn execute_leave(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    // If user wants to leave, we return any pending joining assets and add him in the list for leaving the vault if he has active assets in it
    let mut response = Response::new();

    if let Some(funds) =
        ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, info.sender.to_owned())?
    {
        // We return the pending joining assets
        let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
        assets_pending[0].amount -= funds[0].amount;
        assets_pending[1].amount -= funds[1].amount;
        ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

        ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, info.sender.to_owned());
        let send_msg = BankMsg::Send {
            to_address: info.sender.to_string(),
            amount: funds,
        };

        response = response.add_message(send_msg);
    }

    if VAULT_RATIO.has(deps.storage, info.sender.to_owned()) {
        // We add the user to the list of addresses waiting to leave the vault
        let mut addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;
        if addresses_waiting_for_exit.contains(&info.sender) {
            return Err(ContractError::UserAlreadyInExitList {});
        }
        addresses_waiting_for_exit.push(info.sender.to_owned());
        ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &addresses_waiting_for_exit)?;
    }

    Ok(response
        .add_attribute("action", "leave_banana_vault")
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
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let balance_asset1 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset1.denom.to_owned(),
    )?;
    let balance_asset2 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset2.denom.to_owned(),
    )?;

    verify_availability_of_funds(
        deps.storage,
        tokens_provided.clone(),
        balance_asset1,
        balance_asset2,
    )?;

    let msg_add_position: CosmosMsg = MsgCreatePosition {
        pool_id: CONFIG.load(deps.storage)?.pool_id,
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
    .into();

    Ok(Response::new()
        .add_message(msg_add_position)
        .add_attribute("action", "add_position"))
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
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let balance_asset1 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset1.denom.to_owned(),
    )?;
    let balance_asset2 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset2.denom.to_owned(),
    )?;

    let tokens_provided = vec![
        coin(amount0.parse::<u128>()?, config.asset1.denom.to_owned()),
        coin(amount1.parse::<u128>()?, config.asset2.denom.to_owned()),
    ];

    verify_availability_of_funds(
        deps.storage,
        tokens_provided.clone(),
        balance_asset1,
        balance_asset2,
    )?;

    let msg_add_to_position: CosmosMsg = MsgAddToPosition {
        position_id,
        sender: env.contract.address.to_string(),
        amount0,
        amount1,
        token_min_amount0,
        token_min_amount1,
    }
    .into();

    Ok(Response::new()
        .add_message(msg_add_to_position)
        .add_attribute("action", "add_to_position"))
}

fn execute_withdraw_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: u64,
    liquidity_amount: String,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;

    let mut response = Response::new();

    let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
        position_id,
        sender: env.contract.address.to_string(),
        liquidity_amount,
    }
    .into();

    response = response.add_message(msg_withdraw_position);

    let last_update = LAST_UPDATE.load(deps.storage)?;
    let config = CONFIG.load(deps.storage)?;

    // We will only execute this message if it's time for update (it will execute after the withdraw and check if there are any positions open)
    let update_users_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: env.contract.address.to_string(),
        msg: to_json_binary(&ExecuteMsg::ProcessNewEntriesAndExits {})?,
        funds: vec![],
    });

    match config.update_frequency {
        Frequency::Blocks(blocks) => {
            if env.block.height < last_update + blocks {
                response = response.add_message(update_users_msg)
            }
        }
        Frequency::Seconds(seconds) => {
            if env.block.time.seconds() < last_update + seconds {
                response = response.add_message(update_users_msg)
            }
        }
    }

    Ok(response.add_attribute("action", "withdraw_position"))
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
    let user_positions_response =
        cl_querier.user_positions(env.contract.address.to_string(), config.pool_id, None)?;

    let mut messages = vec![];
    let mut response = Response::new();
    if user_positions_response.positions.is_empty() {
        // These are all the assets the vault has that are not pending to join
        let balance_asset1 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset1.denom.to_owned(),
        )?;
        let balance_asset2 = deps
            .querier
            .query_balance(env.contract.address, config.asset2.denom.to_owned())?;

        let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

        let available_in_vault_asset1 = coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        );

        let available_in_vault_asset2 = coin(
            balance_asset2
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset2.denom.to_owned(),
        );

        let addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;

        let mut total_commission_asset1 = Uint128::zero();
        let mut total_commission_asset2 = Uint128::zero();
        // We are going to process all exits first
        for exit_address in addresses_waiting_for_exit.iter() {
            let ratio = VAULT_RATIO.load(deps.storage, exit_address.to_owned())?;
            let mut amount_to_send_asset1 = available_in_vault_asset1.amount.mul_floor(ratio);
            let mut amount_to_send_asset2 = available_in_vault_asset2.amount.mul_floor(ratio);
            // If there is a vault exit commission, we will take it from the amount to send
            if let Some(exit_commission) = config.exit_commission {
                let amount_commission_asset1 = amount_to_send_asset1.mul_floor(exit_commission);
                let amount_commission_asset2 = amount_to_send_asset2.mul_floor(exit_commission);

                amount_to_send_asset1 =
                    amount_to_send_asset1.checked_sub(amount_commission_asset1)?;
                amount_to_send_asset2 =
                    amount_to_send_asset2.checked_sub(amount_commission_asset2)?;

                total_commission_asset1 =
                    total_commission_asset1.checked_add(amount_commission_asset1)?;
                total_commission_asset2 =
                    total_commission_asset2.checked_add(amount_commission_asset2)?;
            }

            let mut amounts_send_msg = vec![];
            if amount_to_send_asset1.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset1.u128(),
                    config.asset1.denom.to_owned(),
                ));
            }

            if amount_to_send_asset2.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset2.u128(),
                    config.asset2.denom.to_owned(),
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

        // Add the commission message for vault owner
        let owner = get_ownership(deps.storage)?;
        let mut coins_for_owner = vec![];
        if total_commission_asset1.gt(&Uint128::zero()) {
            coins_for_owner.push(coin(
                total_commission_asset1.u128(),
                config.asset1.denom.to_owned(),
            ));
        }
        if total_commission_asset2.gt(&Uint128::zero()) {
            coins_for_owner.push(coin(
                total_commission_asset2.u128(),
                config.asset2.denom.to_owned(),
            ));
        }

        if !coins_for_owner.is_empty() {
            let send_msg = BankMsg::Send {
                to_address: owner.owner.unwrap().to_string(),
                amount: coins_for_owner,
            };

            messages.push(send_msg);
        }

        ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &vec![])?;
        // After processing all exits, we are going to see how much the previous vault participants and new participants own (in dollars) to recalculate vault ratios
        // Get prices in dollars for each asset
        let price_feed_response_asset1: PriceFeedResponse = query_price_feed(
            &deps.querier,
            config.pyth_contract_address.to_owned(),
            config.asset1.identifier,
        )?;
        let price_feed_asset1 = price_feed_response_asset1.price_feed;
        let current_price_asset1 = price_feed_asset1.get_price_unchecked().price;

        let price_feed_response_asset2: PriceFeedResponse = query_price_feed(
            &deps.querier,
            config.pyth_contract_address,
            config.asset2.identifier,
        )?;
        let price_feed_asset2 = price_feed_response_asset2.price_feed;
        let current_price_asset2 = price_feed_asset2.get_price_unchecked().price;

        // Now that we have current prices of both assets we are going to see how much each address owns in dollars and store it to recalculate vault ratios
        let mut total_dollars_in_vault = Uint128::zero();
        let mut address_dollars_map: HashMap<Addr, Uint128> = HashMap::new();

        let ratios: Vec<(Addr, Decimal)> = VAULT_RATIO
            .range(deps.storage, None, None, Order::Ascending)
            .filter_map(|v| v.ok())
            .map(|(a, v)| (a, v))
            .collect();

        for ratio in ratios.iter() {
            let mut amount_assets1 = available_in_vault_asset1.amount.mul_floor(ratio.1);
            let mut amount_assets2 = available_in_vault_asset2.amount.mul_floor(ratio.1);

            // If for some reason this address is actually waiting to join with more assets, we will add this here too
            if let Some(funds) =
                ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, ratio.0.to_owned())?
            {
                amount_assets1 += funds[0].amount;
                amount_assets2 += funds[1].amount;
                // Remove it from here to avoid processing it again later
                ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, ratio.0.to_owned())
            }

            let dollars_asset1 =
                amount_assets1.checked_mul(Uint128::new(current_price_asset1 as u128))?;
            let dollars_asset2 =
                amount_assets2.checked_mul(Uint128::new(current_price_asset2 as u128))?;

            let total_amount_dollars_for_address = dollars_asset1.checked_add(dollars_asset2)?;

            total_dollars_in_vault =
                total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

            address_dollars_map.insert(ratio.0.to_owned(), total_amount_dollars_for_address);
        }

        // Now we are going to process all the new entries
        let new_entries: Vec<(Addr, Vec<Coin>)> = ACCOUNTS_PENDING_ACTIVATION
            .range(deps.storage, None, None, Order::Ascending)
            .filter_map(|v| v.ok())
            .map(|(a, v)| (a, v))
            .collect();

        for new_entry in new_entries.iter() {
            let dollars_asset1 = new_entry.1[0]
                .amount
                .checked_mul(Uint128::new(current_price_asset1 as u128))?;
            let dollars_asset2 = new_entry.1[1]
                .amount
                .checked_mul(Uint128::new(current_price_asset2 as u128))?;

            let total_amount_dollars_for_address = dollars_asset1.checked_add(dollars_asset2)?;

            total_dollars_in_vault =
                total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

            address_dollars_map.insert(new_entry.0.to_owned(), total_amount_dollars_for_address);
        }

        // Now we have all addresses with their corresponding amount in dollars and the total amount of dollars in the vault, let's recalculate ratios

        // Clear the map
        VAULT_RATIO.clear(deps.storage);

        // Recalculate ratios
        for each_address in address_dollars_map.iter() {
            let ratio = Decimal::from_ratio(each_address.1.to_owned(), total_dollars_in_vault);
            VAULT_RATIO.save(deps.storage, each_address.0.to_owned(), &ratio)?;
        }

        // Clean all pending activation addresses and amount
        ACCOUNTS_PENDING_ACTIVATION.clear(deps.storage);
        ASSETS_PENDING_ACTIVATION.save(
            deps.storage,
            &vec![coin(0, config.asset1.denom), coin(0, config.asset2.denom)],
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

        // All bank messages that need to be sent
        response = response.add_messages(messages)
    }

    Ok(response.add_attribute("action", "process_new_entries_and_exits"))
}

fn execute_swap_exact_amount_in(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    routes: Vec<SwapAmountInRoute>,
    token_in: Coin,
    token_out_min_amount: String,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    // We are not allowed to use more than what we have currently available in the vault
    if token_in.denom == config.asset1.denom {
        let balance_asset1 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset1.denom.to_owned(),
        )?;

        let available_to_swap_asset1 = coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        );

        if token_in.amount > available_to_swap_asset1.amount {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    if token_in.denom == config.asset2.denom {
        let balance_asset2 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset2.denom.to_owned(),
        )?;

        let available_to_swap_asset2 = coin(
            balance_asset2
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset2.denom.to_owned(),
        );

        if token_in.amount > available_to_swap_asset2.amount {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    let msg_swap_exact_amount_in: CosmosMsg = MsgSwapExactAmountIn {
        sender: env.contract.address.to_string(),
        routes,
        token_in: Some(CosmosCoin {
            denom: token_in.denom,
            amount: token_in.amount.to_string(),
        }),
        token_out_min_amount,
    }
    .into();

    Ok(Response::new()
        .add_message(msg_swap_exact_amount_in)
        .add_attribute("action", "swap_exact_amount_in"))
}

fn execute_split_route_swap_exact_amount_in(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    routes: Vec<SwapAmountInSplitRoute>,
    token_in_denom: String,
    token_out_min_amount: String,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    let mut token_in_amount = 0;
    for route in routes.iter() {
        token_in_amount += route.token_in_amount.parse::<u128>()?;
    }

    // We are not allowed to use more than what we have currently available in the vault
    if token_in_denom == config.asset1.denom {
        let balance_asset1 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset1.denom.to_owned(),
        )?;

        let available_to_swap_asset1 = coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        );

        if token_in_amount > available_to_swap_asset1.amount.u128() {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    if token_in_denom == config.asset2.denom {
        let balance_asset2 = deps.querier.query_balance(
            env.contract.address.to_owned(),
            config.asset2.denom.to_owned(),
        )?;

        let available_to_swap_asset2 = coin(
            balance_asset2
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset2.denom.to_owned(),
        );

        if token_in_amount > available_to_swap_asset2.amount.u128() {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
    }

    let msg_split_route_swap_exact_amount_in: CosmosMsg = MsgSplitRouteSwapExactAmountIn {
        sender: env.contract.address.to_string(),
        routes,
        token_in_denom,
        token_out_min_amount,
    }
    .into();

    Ok(Response::new()
        .add_message(msg_split_route_swap_exact_amount_in)
        .add_attribute("action", "swap_split_route_swap_exact_amount_in"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::Ownership {} => to_json_binary(&get_ownership(deps.storage)?),
        QueryMsg::ActiveVaultAssets {} => to_json_binary(&query_active_vault_assets(deps, env)?),
        QueryMsg::PendingJoin { address } => to_json_binary(&query_pending_join(deps, address)?),
        QueryMsg::VaultRatio { address } => to_json_binary(&query_vault_ratio(deps, address)?),
    }
}

fn query_config(deps: Deps) -> StdResult<Config> {
    let config = CONFIG.load(deps.storage)?;
    Ok(config)
}

fn query_active_vault_assets(deps: Deps, env: Env) -> StdResult<ActiveVaultAssetsResponse> {
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
    let config = CONFIG.load(deps.storage)?;

    let balance_asset1 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset1.denom.to_owned(),
    )?;
    let balance_asset2 = deps
        .querier
        .query_balance(env.contract.address, config.asset2.denom.to_owned())?;

    Ok(ActiveVaultAssetsResponse {
        asset1: coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset1.denom,
        ),
        asset2: coin(
            balance_asset2
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset2.denom,
        ),
    })
}

fn query_pending_join(deps: Deps, address: Addr) -> StdResult<Vec<Coin>> {
    let assets = ACCOUNTS_PENDING_ACTIVATION.load(deps.storage, address)?;
    Ok(assets)
}

fn query_vault_ratio(deps: Deps, address: Addr) -> StdResult<Decimal> {
    let ratio = VAULT_RATIO.load(deps.storage, address)?;
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
    if config.asset1.denom != pool.token0 && config.asset1.denom != pool.token1 {
        return Err(ContractError::InvalidConfigAsset {});
    }

    if config.asset2.denom != pool.token0 && config.asset2.denom != pool.token1 {
        return Err(ContractError::InvalidConfigAsset {});
    }

    Ok(())
}

fn verify_funds(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    if info.funds.len() != 2 || info.funds[0].denom == info.funds[1].denom {
        return Err(ContractError::NeedTwoDenoms {});
    }

    if info.funds[0].denom != config.asset1.denom && info.funds[1].denom != config.asset2.denom {
        return Err(ContractError::InvalidFunds {});
    }

    Ok(())
}

fn verify_availability_of_funds(
    storage: &mut dyn Storage,
    tokens_provided: Vec<Coin>,
    balance_asset1: Coin,
    balance_asset2: Coin,
) -> Result<(), ContractError> {
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(storage)?;
    let config = CONFIG.load(storage)?;

    let available_to_add_asset1 = coin(
        balance_asset1
            .amount
            .checked_sub(assets_pending[0].amount)?
            .u128(),
        config.asset1.denom.to_owned(),
    );

    let available_to_add_asset2 = coin(
        balance_asset2
            .amount
            .checked_sub(assets_pending[1].amount)?
            .u128(),
        config.asset2.denom.to_owned(),
    );

    for token_provided in tokens_provided.iter() {
        if token_provided.denom == config.asset1.denom
            && token_provided.amount > available_to_add_asset1.amount
        {
            return Err(ContractError::CannotAddMoreThenAvailableForAsset {
                asset: config.asset1.denom,
            });
        }
        if token_provided.denom == config.asset2.denom
            && token_provided.amount > available_to_add_asset2.amount
        {
            return Err(ContractError::CannotAddMoreThenAvailableForAsset {
                asset: config.asset2.denom,
            });
        }
    }

    Ok(())
}
