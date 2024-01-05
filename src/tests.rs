#[cfg(test)]
mod tests {
    use cosmwasm_std::{coin, Coin, Decimal, Uint128, Addr};
    use osmosis_std::types::{
        cosmos::{bank::v1beta1::QueryBalanceRequest, base::v1beta1::Coin as BankCoin},
        osmosis::{
            concentratedliquidity::{
                poolmodel::concentrated::v1beta1::MsgCreateConcentratedPool, v1beta1::ParamsRequest,
            },
            poolmanager::v1beta1::AllPoolsRequest,
            tokenfactory::v1beta1::{MsgCreateDenom, MsgMint, QueryDenomsFromCreatorRequest},
        },
    };
    use osmosis_test_tube::{
        cosmrs::proto::cosmos::params::v1beta1::{ParamChange, ParameterChangeProposal},
        Account, Bank, ConcentratedLiquidity, Gamm, GovWithAppAccess, Module, OsmosisTestApp,
        PoolManager, SigningAccount, TokenFactory, Wasm,
    };
    use pyth_sdk_cw::PriceIdentifier;

    use crate::{
        error::ContractError,
        msg::{Frequency, InstantiateMsg, PythAsset},
    };

    const FEE_DENOM: &str = "uosmo";

    fn store_and_instantiate_vault(
        wasm: &Wasm<'_, OsmosisTestApp>,
        signer: &SigningAccount,
        pool_id: u64,
        update_frequency: Frequency,
        asset1: PythAsset,
        asset2: PythAsset,
        dollar_cap: Option<u64>,
        exit_commission: Option<Decimal>,
        commission_receiver: Option<Addr>,
        funds: Vec<Coin>,
    ) -> String {
        let wasm_byte_code = std::fs::read("./artifacts/banana_vault.wasm").unwrap();
        let code_id = wasm
            .store_code(&wasm_byte_code, None, &signer)
            .unwrap()
            .data
            .code_id;
        wasm.instantiate(
            code_id,
            &InstantiateMsg {
                pool_id,
                update_frequency,
                asset1,
                asset2,
                dollar_cap,
                commission_receiver,
                mainnet: false,
                exit_commission,
            },
            None,
            "banana-vault".into(),
            &funds,
            &signer,
        )
        .unwrap()
        .data
        .address
    }

    fn create_token_and_whitelist(
        tf: &TokenFactory<'_, OsmosisTestApp>,
        gov: &GovWithAppAccess,
        cl: &ConcentratedLiquidity<'_, OsmosisTestApp>,
        signer: &SigningAccount,
        name: String,
    ) -> String {
        tf.create_denom(
            MsgCreateDenom {
                sender: signer.address(),
                subdenom: name.to_owned(),
            },
            &signer,
        )
        .unwrap();

        let denom = format!("factory/{}/{}", signer.address(), name.as_str()).to_lowercase();
        let query_denoms = tf
            .query_denoms_from_creator(&QueryDenomsFromCreatorRequest {
                creator: signer.address(),
            })
            .unwrap();

        assert_eq!(query_denoms.denoms.len(), 1);
        assert_eq!(query_denoms.denoms[0], denom);

        // Get whitelisted assets
        let mut whitelisted_assets = cl
            .query_params(&ParamsRequest {})
            .unwrap()
            .params
            .unwrap()
            .authorized_quote_denoms;

        whitelisted_assets.push(denom.to_owned());

        gov.propose_and_execute(
            "/cosmos.params.v1beta1.ParameterChangeProposal".to_string(),
            ParameterChangeProposal {
                title: "Enable Permissionless Creation of Supercharged Pools".to_string(),
                description: "LFG".to_string(),
                changes: vec![ParamChange {
                    subspace: "concentratedliquidity".to_string(),
                    key: "IsPermisionlessPoolCreationEnabled".to_string(),
                    value: "true".to_string(),
                }],
            },
            signer.address(),
            &signer,
        )
        .unwrap();

        let assets = whitelisted_assets.join(r#"",""#);
        let value = r#"[""#.to_string() + &assets + r#"""# + "]";

        gov.propose_and_execute(
            "/cosmos.params.v1beta1.ParameterChangeProposal".to_string(),
            ParameterChangeProposal {
                title: "Whitelist Asset".to_string(),
                description: "LFG".to_string(),
                changes: vec![ParamChange {
                    subspace: "poolmanager".to_string(),
                    key: "AuthorizedQuoteDenoms".to_string(),
                    value,
                }],
            },
            signer.address(),
            &signer,
        )
        .unwrap();

        denom
    }

    #[test]
    fn contract() {
        let app = OsmosisTestApp::new();
        let signer = app
            .init_account(&[coin(100_000_000_000, FEE_DENOM)])
            .unwrap();
        let cl = ConcentratedLiquidity::new(&app);
        let tf = TokenFactory::new(&app);
        let gov = GovWithAppAccess::new(&app);
        let wasm = Wasm::new(&app);
        let pm = PoolManager::new(&app);
        let gamm = Gamm::new(&app);
        let bank = Bank::new(&app);

        let subdenom = "banana".to_string();
        let denom = create_token_and_whitelist(&tf, &gov, &cl, &signer, subdenom);

        // Mint some banana token to ourselves
        tf.mint(
            MsgMint {
                sender: signer.address(),
                amount: Some(BankCoin {
                    denom: denom.to_owned(),
                    amount: "1000000000000".to_string(),
                }),
                mint_to_address: signer.address(),
            },
            &signer,
        )
        .unwrap();

        let balance_response = bank
            .query_balance(&QueryBalanceRequest {
                address: signer.address(),
                denom: denom.to_owned(),
            })
            .unwrap();

        assert_eq!(balance_response.balance.unwrap().amount, "1000000000000");

        // Let's create a Banana/Osmo CL Pool
        cl.create_concentrated_pool(
            MsgCreateConcentratedPool {
                sender: signer.address(),
                denom0: "uosmo".to_string(),
                denom1: denom.to_owned(),
                tick_spacing: 1,
                spread_factor: "0".to_string(),
            },
            &signer,
        )
        .unwrap();

        // Let's also create a basic Banana/Osmo pool
        gamm.create_basic_pool(
            &vec![
                Coin {
                    denom: FEE_DENOM.to_string(),
                    amount: Uint128::one(),
                },
                Coin {
                    denom: denom.to_owned(),
                    amount: Uint128::one(),
                },
            ],
            &signer,
        )
        .unwrap();

        let query_response = pm.query_all_pools(&AllPoolsRequest {}).unwrap();

        assert_eq!(query_response.pools.len(), 2);
        assert_eq!(
            query_response.pools[0].type_url,
            "/osmosis.concentratedliquidity.v1beta1.Pool".to_string()
        );

        let uosmo_identifier = PriceIdentifier::from_hex(
            "5867f5683c757393a0670ef0f701490950fe93fdb006d181c8265a831ac0c5c6".to_string(),
        )
        .unwrap();
        // Random identifier for our banana token for testing purposes
        let banana_identifier = PriceIdentifier::from_hex(
            "b00b60f88b03a6a625a8d1c048c3f66653edf217439983d037e7222c4e612819".to_string(),
        )
        .unwrap();

        store_and_instantiate_vault(
            &wasm,
            &signer,
            1,
            Frequency::Seconds(1000),
            PythAsset {
                denom: denom.to_owned(),
                identifier: banana_identifier.to_owned(),
            },
            PythAsset {
                denom: "uosmo".to_string(),
                identifier: uosmo_identifier.to_owned(),
            },
            None,
            Some(Decimal::percent(50)),
            None,
            vec![
                coin(100_000_000, denom.to_owned()),
                coin(100_000_000, FEE_DENOM),
            ],
        );

        // Trying to instantiate a banana vault with a non CL pool should fail
        let instantiation_error = wasm
            .instantiate(
                1,
                &InstantiateMsg {
                    pool_id: 2,
                    update_frequency: Frequency::Seconds(1000),
                    asset1: PythAsset {
                        denom: "uosmo".to_string(),
                        identifier: uosmo_identifier.to_owned(),
                    },
                    asset2: PythAsset {
                        denom: denom.to_owned(),
                        identifier: banana_identifier.to_owned(),
                    },
                    dollar_cap: None,
                    exit_commission: Some(Decimal::percent(50)),
                    commission_receiver: None,
                    mainnet: false,
                },
                None,
                "banana-vault".into(),
                &vec![
                    coin(100_000_000, denom.to_owned()),
                    coin(100_000_000, FEE_DENOM),
                ],
                &signer,
            )
            .unwrap_err();

        assert!(instantiation_error
            .to_string()
            .contains(ContractError::PoolIsNotCL {}.to_string().as_str()));

        // Trying to instantiate a banana vault with invalid assets in config should should fail
        let instantiation_error = wasm
            .instantiate(
                1,
                &InstantiateMsg {
                    pool_id: 1,
                    update_frequency: Frequency::Seconds(1000),
                    asset1: PythAsset {
                        denom: "uosmo".to_string(),
                        identifier: uosmo_identifier.to_owned(),
                    },
                    asset2: PythAsset {
                        denom: "uatom".to_owned(),
                        identifier: banana_identifier.to_owned(),
                    },
                    dollar_cap: None,
                    exit_commission: Some(Decimal::percent(50)),
                    commission_receiver: None,
                    mainnet: false,
                },
                None,
                "banana-vault".into(),
                &vec![
                    coin(100_000_000, denom.to_owned()),
                    coin(100_000_000, FEE_DENOM),
                ],
                &signer,
            )
            .unwrap_err();

        assert!(instantiation_error
            .to_string()
            .contains(ContractError::InvalidConfigAsset {}.to_string().as_str()));

        // Trying to instantiate a banana vault sending wrong assets should fail
        let instantiation_error = wasm
            .instantiate(
                1,
                &InstantiateMsg {
                    pool_id: 1,
                    update_frequency: Frequency::Seconds(1000),
                    asset1: PythAsset {
                        denom: "uosmo".to_string(),
                        identifier: uosmo_identifier.to_owned(),
                    },
                    asset2: PythAsset {
                        denom: denom.to_owned(),
                        identifier: banana_identifier.to_owned(),
                    },
                    dollar_cap: None,
                    exit_commission: Some(Decimal::percent(50)),
                    commission_receiver: None,
                    mainnet: false,
                },
                None,
                "banana-vault".into(),
                &vec![coin(100_000_000, FEE_DENOM)],
                &signer,
            )
            .unwrap_err();

        assert!(instantiation_error
            .to_string()
            .contains(ContractError::NeedTwoDenoms {}.to_string().as_str()));
    }
}
