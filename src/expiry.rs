// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! Ex8:Margin binomial Greeks + futures expiry engine.
//!
//! - **Binomial**: Cox-Ross-Rubinstein American option pricing +
//!   Greeks — the exercise-early premium path the BSM closed form cannot
//!   price (American puts / high-rate environments); see
//!   `services/margin/DESIGN.md` §2 (pricing) and §7 (tree internals).
//! - **Expiry & settlement engine**: futures contract expiry
//!   `open → expiring → settled` state machine — final settlement price
//!   (index average or last), position cash settlement at expiry, and the
//!   physical delivery obligation alternative; see `services/margin/DESIGN.md`
//!   §2 (expiry processing) and §3 (expiry state machine).
//!
//! Design: `design/sentin8-exchange8-options-technical-design.md` §3.2,
//! `design/sentin8-exchange8-futures-technical-design.md` §3 (expiry).

use crate::greeks::{Greeks, OptionType};

// ---------------------------------------------------------------------------
// CRR binomial pricing + Greeks (American exercise) — DESIGN.md §7
// ---------------------------------------------------------------------------

/// Cox-Ross-Rubinstein binomial tree parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinomialParams {
    /// Steps in the tree (CRR convergence; 100+ for production).
    pub steps: u32,
    /// Risk-free rate (annualized, e.g. 0.05 = 5%).
    pub r: f64,
    /// Annualized volatility (e.g. 0.2 = 20%).
    pub sigma: f64,
    /// Time to expiry in years.
    pub t: f64,
}

/// American option price via the CRR binomial tree (O(N²)) — see
/// `services/margin/DESIGN.md` §7 (CRR tree).
///
/// u = e^{σ√dt}, d = 1/u, p = (e^{r·dt} − d)/(u − d); early-exercise value
/// taken at every node (max(intrinsic, continuation)).
pub fn crr_price_american(
    option_type: OptionType,
    spot: f64,
    strike: f64,
    p: &BinomialParams,
) -> f64 {
    let n = p.steps.max(1) as usize;
    let dt = p.t / n as f64;
    let u = (p.sigma * dt.sqrt()).exp();
    let d = 1.0 / u;
    let disc = (-p.r * dt).exp();
    let growth = (p.r * dt).exp();
    let prob = ((growth - d) / (u - d)).clamp(0.0, 1.0);

    // Terminal payoffs.
    let mut vals: Vec<f64> = (0..=n)
        .map(|i| {
            let s = spot * u.powi(i as i32) * d.powi((n - i) as i32);
            match option_type {
                OptionType::Call => (s - strike).max(0.0),
                OptionType::Put => (strike - s).max(0.0),
            }
        })
        .collect();

    // Backward induction with early exercise.
    for step in (0..n).rev() {
        for i in 0..=step {
            let s = spot * u.powi(i as i32) * d.powi((step - i) as i32);
            let cont = disc * (prob * vals[i + 1] + (1.0 - prob) * vals[i]);
            let intrinsic = match option_type {
                OptionType::Call => (s - strike).max(0.0),
                OptionType::Put => (strike - s).max(0.0),
            };
            vals[i] = cont.max(intrinsic);
        }
    }
    vals[0]
}

/// Greeks from the binomial tree by finite differences (matches the BSM
/// `Greeks` struct so risk consumers use one shape — DESIGN.md §7).
pub fn crr_greeks_american(
    option_type: OptionType,
    spot: f64,
    strike: f64,
    p: &BinomialParams,
) -> Greeks {
    let base = crr_price_american(option_type, spot, strike, p);
    // Delta/price bumps.
    let h_s = spot * 0.001;
    let up = crr_price_american(option_type, spot + h_s, strike, p);
    let dn = crr_price_american(option_type, spot - h_s, strike, p);
    let delta = (up - dn) / (2.0 * h_s);
    let gamma = (up - 2.0 * base + dn) / (h_s * h_s);
    // Theta: shorten time by a small fraction.
    let h_t = p.t.max(1.0 / 365.0) * (1.0 / 100.0);
    let shorter = crr_price_american(
        option_type,
        spot,
        strike,
        &BinomialParams {
            t: (p.t - h_t).max(0.0),
            ..*p
        },
    );
    let theta = (shorter - base) / h_t; // per year
                                        // Vega/theta: bump sigma.
    let h_v = 0.0001;
    let vol_up = crr_price_american(
        option_type,
        spot,
        strike,
        &BinomialParams {
            sigma: p.sigma + h_v,
            ..*p
        },
    );
    let vega = (vol_up - base) / h_v;
    // Rho: bump r.
    let h_r = 0.0001;
    let r_up = crr_price_american(
        option_type,
        spot,
        strike,
        &BinomialParams { r: p.r + h_r, ..*p },
    );
    let rho = (r_up - base) / h_r;
    Greeks {
        price: base,
        delta,
        gamma,
        vega,
        theta,
        rho,
    }
}

// ---------------------------------------------------------------------------
// Futures expiry & settlement — DESIGN.md §2 (expiry processing lifecycle)
// ---------------------------------------------------------------------------

/// Contract settlement style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuturesSettlement {
    Cash,
    Physical,
}

/// Expiry lifecycle state (`open → expiring → settled`; DESIGN.md §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpiryState {
    Open,
    /// Expiry reached; settlement price being formed (average window).
    Expiring,
    Settled,
}

/// One contract up for expiry.
#[derive(Clone, Debug)]
pub struct ExpiringContract {
    pub contract_id: u32,
    pub settlement: FuturesSettlement,
    pub state: ExpiryState,
}

/// A settlement-price observation during the expiry average window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettlementObs {
    pub ts_us: u64,
    pub price_raw: i128,
}

/// The expiry engine: collects the settlement average, then settles
/// positions.
pub struct ExpiryEngine {
    /// Contracts in the expiry flow.
    contracts: std::collections::HashMap<u32, ExpiringContract>,
    /// Settlement-price observations per contract.
    obs: std::collections::HashMap<u32, Vec<SettlementObs>>,
    /// Position snapshot per (contract, account): signed qty + entry.
    positions: std::collections::HashMap<(u32, String), (i64, i128)>,
    /// Settlement results per contract.
    settled: std::collections::HashMap<u32, ContractSettlement>,
}

/// The settlement outcome for one contract.
#[derive(Clone, Debug, PartialEq)]
pub struct ContractSettlement {
    pub contract_id: u32,
    /// Final settlement price (raw) — the average of observations.
    pub settlement_price_raw: i128,
    /// Cash settlement P&L per account (mark-to-settlement vs entry).
    pub cash_pnl: Vec<(String, i128)>,
    /// Physical delivery obligations (account, signed qty).
    pub deliveries: Vec<(String, i64)>,
    pub state: ExpiryState,
}

impl ExpiryEngine {
    pub fn new() -> Self {
        ExpiryEngine {
            contracts: std::collections::HashMap::new(),
            obs: std::collections::HashMap::new(),
            positions: std::collections::HashMap::new(),
            settled: std::collections::HashMap::new(),
        }
    }

    /// Register a contract entering its expiry window.
    pub fn enter_expiry(&mut self, contract_id: u32, settlement: FuturesSettlement) {
        self.contracts.insert(
            contract_id,
            ExpiringContract {
                contract_id,
                settlement,
                state: ExpiryState::Expiring,
            },
        );
    }

    /// Load one account's position on a contract (projection).
    pub fn load_position(&mut self, contract_id: u32, account: &str, qty: i64, entry_raw: i128) {
        self.positions
            .insert((contract_id, account.into()), (qty, entry_raw));
    }

    /// A settlement-price observation (average window input).
    pub fn observe(&mut self, contract_id: u32, o: SettlementObs) {
        self.obs.entry(contract_id).or_default().push(o);
    }

    /// Settle one contract: average price → cash P&L or physical delivery.
    /// Idempotent: a second call returns the recorded settlement.
    pub fn settle(&mut self, contract_id: u32) -> Option<ContractSettlement> {
        if let Some(s) = self.settled.get(&contract_id) {
            return Some(s.clone());
        }
        let contract = self.contracts.get(&contract_id)?;
        if contract.state != ExpiryState::Expiring {
            return None;
        }
        // Settlement price = arithmetic mean of observations.
        let obs = self.obs.get(&contract_id)?;
        if obs.is_empty() {
            return None;
        }
        let mean = obs.iter().map(|o| o.price_raw).sum::<i128>() / obs.len() as i128;

        // Collect this contract's positions.
        let keys: Vec<(u32, String)> = self
            .positions
            .keys()
            .filter(|(c, _)| *c == contract_id)
            .cloned()
            .collect();
        let mut cash_pnl = Vec::new();
        let mut deliveries = Vec::new();
        for (_, account) in &keys {
            let (qty, entry) = self.positions[&(contract_id, account.clone())];
            match contract.settlement {
                FuturesSettlement::Cash => {
                    // P&L = qty × (settlement − entry).
                    cash_pnl.push((account.clone(), qty as i128 * (mean - entry)));
                }
                FuturesSettlement::Physical => {
                    // Long receives, short delivers.
                    deliveries.push((account.clone(), qty));
                }
            }
        }
        let result = ContractSettlement {
            contract_id,
            settlement_price_raw: mean,
            cash_pnl,
            deliveries,
            state: ExpiryState::Settled,
        };
        // Terminal: mark contract settled, drop positions.
        if let Some(c) = self.contracts.get_mut(&contract_id) {
            c.state = ExpiryState::Settled;
        }
        self.positions.retain(|(c, _), _| *c != contract_id);
        self.settled.insert(contract_id, result.clone());
        Some(result)
    }

    pub fn state(&self, contract_id: u32) -> Option<ExpiryState> {
        self.contracts.get(&contract_id).map(|c| c.state)
    }
}

impl Default for ExpiryEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BinomialParams {
        BinomialParams {
            steps: 200,
            r: 0.05,
            sigma: 0.2,
            t: 1.0,
        }
    }

    #[test]
    fn crr_european_converges_to_bsm() {
        // With early exercise worthless (call, no dividends), CRR ≈ BSM.
        let p = params();
        let crr = crr_price_american(OptionType::Call, 100.0, 100.0, &p);
        // BSM call: S·N(d1) − K·e^{−rT}·N(d2) ≈ 10.4506.
        let bsm = 10.4506;
        assert!(
            (crr - bsm).abs() < 0.05,
            "CRR {crr} should converge to BSM {bsm}"
        );
    }

    #[test]
    fn crr_american_put_at_least_european() {
        let p = params();
        let american = crr_price_american(OptionType::Put, 100.0, 100.0, &p);
        // European put BSM ≈ 5.5735; American ≥ European.
        let european = 5.5735;
        assert!(
            american >= european - 0.01,
            "American put {american} ≥ European {european}"
        );
        // Deep ITM American put ≈ intrinsic (early exercise optimal).
        let deep = crr_price_american(OptionType::Put, 50.0, 100.0, &p);
        assert!(deep > 49.0, "deep ITM put ≈ K−S = 50, got {deep}");
    }

    #[test]
    fn crr_greeks_delta_signs() {
        let p = params();
        let g = crr_greeks_american(OptionType::Call, 100.0, 100.0, &p);
        assert!(
            g.delta > 0.0 && g.delta < 1.0,
            "call delta in (0,1): {}",
            g.delta
        );
        let gp = crr_greeks_american(OptionType::Put, 100.0, 100.0, &p);
        assert!(
            gp.delta < 0.0 && gp.delta > -1.0,
            "put delta in (−1,0): {}",
            gp.delta
        );
        // Put-call parity holds exactly for Europeans; the American put's
        // early-exercise boundary bends delta, so parity degrades to a
        // bound here (deviation ≈ 0.05 for these parameters).
        assert!((g.delta - gp.delta - 1.0).abs() < 0.1);
        // Gamma positive for both.
        assert!(g.gamma > 0.0 && gp.gamma > 0.0);
        // Theta negative for the ATM call (time decay).
        assert!(g.theta < 0.0);
    }

    #[test]
    fn futures_expiry_cash_settles_pnl() {
        let mut e = ExpiryEngine::new();
        e.enter_expiry(7, FuturesSettlement::Cash);
        e.load_position(7, "A", 10, 100_000); // long 10 @ 100_000
        e.load_position(7, "B", -10, 100_000); // short 10 @ 100_000
                                               // Settlement average of 101_000 and 103_000 = 102_000.
        e.observe(
            7,
            SettlementObs {
                ts_us: 1,
                price_raw: 101_000,
            },
        );
        e.observe(
            7,
            SettlementObs {
                ts_us: 2,
                price_raw: 103_000,
            },
        );
        let s = e.settle(7).unwrap();
        assert_eq!(s.settlement_price_raw, 102_000);
        assert_eq!(s.state, ExpiryState::Settled);
        // Long gains 10 × 2_000 = 20_000; short loses the same. (Order is
        // HashMap-derived — compare sorted.)
        let mut pnl = s.cash_pnl.clone();
        pnl.sort();
        assert_eq!(pnl, vec![("A".into(), 20_000), ("B".into(), -20_000)]);
        // Idempotent.
        let s2 = e.settle(7).unwrap();
        assert_eq!(s2, s);
        assert_eq!(e.state(7), Some(ExpiryState::Settled));
    }

    #[test]
    fn futures_expiry_physical_delivers() {
        let mut e = ExpiryEngine::new();
        e.enter_expiry(8, FuturesSettlement::Physical);
        e.load_position(8, "A", 5, 100_000);
        e.load_position(8, "B", -5, 100_000);
        e.observe(
            8,
            SettlementObs {
                ts_us: 1,
                price_raw: 100_000,
            },
        );
        let s = e.settle(8).unwrap();
        assert!(s.cash_pnl.is_empty());
        // Long receives +5, short delivers −5. (HashMap order — sorted.)
        let mut dels = s.deliveries.clone();
        dels.sort();
        assert_eq!(dels, vec![("A".into(), 5), ("B".into(), -5)]);
    }

    #[test]
    fn settle_without_observations_fails() {
        let mut e = ExpiryEngine::new();
        e.enter_expiry(9, FuturesSettlement::Cash);
        assert!(e.settle(9).is_none());
        assert_eq!(e.state(9), Some(ExpiryState::Expiring));
    }

    #[test]
    fn unknown_contract_rejected() {
        let mut e = ExpiryEngine::new();
        assert!(e.settle(99).is_none());
    }
}
