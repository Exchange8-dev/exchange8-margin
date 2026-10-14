// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Margin
// SPDX-License-Identifier: Apache-2.0

//! Ex8:Margin core — position ledger, MTM, margin, funding, liquidation.
//!
//! Event-sourced position accounting for derivatives (futures, perpetuals,
//! options positions downstream). Pure functions: the shell feeds trade
//! events and index ticks, reads margin/liquidation/funding decisions.
//!
//! Design: `design/sentin8-exchange8-margin-technical-design.md` §4.1–4.5;
//! detailed design: `services/margin/DESIGN.md` §2 (risk core), §3 (state
//! machines).

/// Signed position with average entry price (raw fixed-point, i128 scale).
///
/// P&L is tracked in *raw notional* units (price ticks × qty) — the shell
/// divides by the instrument scale pair for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Position {
    /// Net signed quantity (+ long / − short).
    pub qty: i128,
    /// Volume-weighted average entry price (raw).
    pub avg_entry: i128,
    /// Realized P&L accumulated on position reductions (raw).
    pub realized_pnl: i128,
}

impl Position {
    pub fn is_flat(&self) -> bool {
        self.qty == 0
    }

    /// Apply a signed fill. Crossing trades realize P&L; additions move the
    /// average; flips realize then rebuild the average.
    pub fn apply_fill(&mut self, signed_qty: i128, price_raw: i128) {
        if signed_qty == 0 {
            return;
        }
        if self.qty == 0 {
            // Fresh position.
            self.qty = signed_qty;
            self.avg_entry = price_raw;
            return;
        }
        let same_direction = self.qty.signum() == signed_qty.signum();
        if same_direction {
            // Addition: weighted average entry.
            let total = self.qty + signed_qty;
            // qty×price summed — note avg_entry is per-unit, so:
            // avg' = (avg × |old| + px × |add|) / (|old| + |add|)
            let old_abs = self.qty.unsigned_abs();
            let add_abs = signed_qty.unsigned_abs();
            let new_abs = old_abs + add_abs;
            // Weighted in absolute-qty space to avoid sign cancellation.
            self.avg_entry = ((self.avg_entry * old_abs as i128) + (price_raw * add_abs as i128))
                / new_abs as i128;
            self.qty = total;
        } else {
            let close_abs = signed_qty.unsigned_abs() as i128;
            let pos_abs = self.qty.unsigned_abs() as i128;
            if close_abs <= pos_abs {
                // Reduction: realize against the average entry.
                let per_unit = price_raw - self.avg_entry;
                // Long + reduction (sell): profit when px > entry.
                // Short + reduction (buy): profit when px < entry → same
                // formula works because qty carries the sign.
                let pnl = per_unit * close_abs * self.qty.signum();
                self.realized_pnl += pnl;
                self.qty += signed_qty;
                if self.qty == 0 {
                    self.avg_entry = 0;
                }
            } else {
                // Flip: realize the full old position, then open the new one.
                let per_unit = price_raw - self.avg_entry;
                let pnl = per_unit * pos_abs * self.qty.signum();
                self.realized_pnl += pnl;
                let new_qty = self.qty + signed_qty;
                self.qty = new_qty;
                self.avg_entry = price_raw;
            }
        }
    }

    /// Unrealized P&L against a mark price (raw).
    pub fn unrealized(&self, mark: i128) -> i128 {
        if self.qty == 0 {
            return 0;
        }
        (mark - self.avg_entry) * self.qty
    }
}

/// Margin-account state for one (account, contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarginState {
    /// Posted collateral (raw cash).
    pub equity: i128,
    pub position: Position,
    /// Maintenance-margin fraction basis points (e.g. 5000 = 50%).
    pub maint_bps: u32,
    /// Initial-margin fraction basis points.
    pub init_bps: u32,
}

/// Margin check outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarginCheck {
    /// Equity covers the maintenance requirement.
    Ok { requirement: i128 },
    /// Equity below maintenance → liquidation engine.
    Liquidate { requirement: i128, shortfall: i128 },
}

impl MarginState {
    /// Mark-price notional requirement = |position| × mark × maint_bps/10⁴.
    pub fn maintenance_requirement(&self, mark: i128) -> i128 {
        let abs = self.position.qty.unsigned_abs() as i128;
        abs * mark * self.maint_bps as i128 / 10_000
    }

    /// Initial requirement for a new position.
    pub fn initial_requirement(&self, mark: i128) -> i128 {
        let abs = self.position.qty.unsigned_abs() as i128;
        abs * mark * self.init_bps as i128 / 10_000
    }

    /// MTM-adjusted equity = equity + unrealized (cash-settled variation).
    pub fn effective_equity(&self, mark: i128) -> i128 {
        self.equity + self.position.unrealized(mark)
    }

    /// Maintenance check at a mark price.
    pub fn check(&self, mark: i128) -> MarginCheck {
        let requirement = self.maintenance_requirement(mark);
        let eff = self.effective_equity(mark);
        if eff < requirement {
            MarginCheck::Liquidate {
                requirement,
                shortfall: requirement - eff,
            }
        } else {
            MarginCheck::Ok { requirement }
        }
    }
}

/// Funding application for one account's position.
///
/// `funding = clamp(premium_index, ±cap) × position_value / 10⁴` where
/// `position_value = qty × mark`. Longs pay when funding is positive
/// (premium above index); shorts receive. Sign flows through qty.
pub fn funding_payment(premium_index_bps: i64, cap_bps: i64, qty: i128, mark: i128) -> i128 {
    let clamped = premium_index_bps.clamp(-cap_bps, cap_bps);
    // Long (qty > 0) pays when premium positive: payment is negative for longs.
    -(clamped as i128) * qty * mark / 10_000
}

/// Liquidation-plan outcome for a breached account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Liquidation {
    /// The account + contract pair breached.
    pub account_id: u64,
    pub contract_id: u32,
    /// Closing order size (|position|, market order to ME).
    pub close_qty: u128,
    /// Side to submit (opposite the position).
    pub close_is_buy: bool,
    /// Equity shortfall at the mark.
    pub shortfall: i128,
    /// Mark price the check ran against.
    pub mark: i128,
}

/// Evaluate one account for liquidation; returns the close plan when breached.
pub fn evaluate_liquidation(
    account_id: u64,
    contract_id: u32,
    state: &MarginState,
    mark: i128,
) -> Option<Liquidation> {
    if state.position.is_flat() {
        return None;
    }
    match state.check(mark) {
        MarginCheck::Ok { .. } => None,
        MarginCheck::Liquidate { shortfall, .. } => Some(Liquidation {
            account_id,
            contract_id,
            close_qty: state.position.qty.unsigned_abs(),
            close_is_buy: state.position.qty < 0,
            shortfall,
            mark,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_position_from_flat() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        assert_eq!((p.qty, p.avg_entry), (100, 10_000));
        assert_eq!(p.realized_pnl, 0);
    }

    #[test]
    fn addition_moves_weighted_average() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        p.apply_fill(100, 10_200);
        assert_eq!(p.qty, 200);
        assert_eq!(p.avg_entry, 10_100);
    }

    #[test]
    fn partial_close_realizes_pnl() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000); // long 100 @ 1.00
        p.apply_fill(-50, 10_400); // sell 50 @ 1.04 → +0.04 × 50
        assert_eq!(p.qty, 50);
        assert_eq!(p.realized_pnl, (10_400 - 10_000) * 50);
        assert_eq!(p.avg_entry, 10_000); // reduction keeps avg
    }

    #[test]
    fn short_close_realizes_pnl() {
        let mut p = Position::default();
        p.apply_fill(-100, 10_000); // short 100 @ 1.00
        p.apply_fill(50, 9_800); // buy 50 @ 0.98 → +0.02 × 50
        assert_eq!(p.qty, -50);
        assert_eq!(p.realized_pnl, (10_000 - 9_800) * 50);
    }

    #[test]
    fn flip_realizes_and_rebuilds() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000); // long 100 @ 1.00
        p.apply_fill(-200, 10_500); // sell 200 → realize +0.05×100, short 100 @ 1.05
        assert_eq!(p.qty, -100);
        assert_eq!(p.realized_pnl, (10_500 - 10_000) * 100);
        assert_eq!(p.avg_entry, 10_500);
    }

    #[test]
    fn close_to_flat_resets_entry() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        p.apply_fill(-100, 10_100);
        assert!(p.is_flat());
        assert_eq!(p.realized_pnl, (10_100 - 10_000) * 100);
        assert_eq!(p.avg_entry, 0);
    }

    #[test]
    fn unrealized_marks_long_short() {
        let mut long = Position::default();
        long.apply_fill(100, 10_000);
        assert_eq!(long.unrealized(10_100), 100 * 100);
        let mut short = Position::default();
        short.apply_fill(-100, 10_000);
        assert_eq!(short.unrealized(9_900), 100 * 100);
        assert_eq!(short.unrealized(10_100), -100 * 100);
    }

    #[test]
    fn maintenance_requirement_and_check() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000); // long 100 @ 1.00
        let s = MarginState {
            equity: 60_000, // 6.00 in raw notional terms of 10⁴ scale
            position: p,
            maint_bps: 5_000, // 50%
            init_bps: 10_000,
        };
        // mark 1.00: requirement = 100 × 10_000 × 50% = 500_000
        let req = s.maintenance_requirement(10_000);
        assert_eq!(req, 500_000);
        // equity 60_000 << 500_000 → liquidate.
        assert!(matches!(s.check(10_000), MarginCheck::Liquidate { .. }));
        // Sufficient equity passes.
        let ok = MarginState {
            equity: 600_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        assert!(matches!(ok.check(10_000), MarginCheck::Ok { .. }));
    }

    #[test]
    fn effective_equity_includes_unrealized() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        let s = MarginState {
            equity: 500_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        // Mark up → unrealized +10_000; equity 500_000 + 10_000.
        assert_eq!(s.effective_equity(10_100), 510_000);
        // Mark down → equity eroded by variation.
        assert_eq!(s.effective_equity(9_900), 490_000);
    }

    #[test]
    fn liquidation_triggered_by_shortfall() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        let breached = MarginState {
            equity: 100_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        // requirement 500_000 > equity 100_000 → liquidation plan.
        let plan = evaluate_liquidation(1, 7, &breached, 10_000).unwrap();
        assert_eq!(plan.close_qty, 100);
        assert!(!plan.close_is_buy); // close long = sell
        assert_eq!(plan.shortfall, 400_000);
        // Well-funded account: no plan.
        let ok = MarginState {
            equity: 600_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        assert!(evaluate_liquidation(1, 7, &ok, 10_000).is_none());
    }

    #[test]
    fn liquidation_plan_closes_short_with_buy() {
        let mut p = Position::default();
        p.apply_fill(-100, 10_000);
        let breached = MarginState {
            equity: 100_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        let plan = evaluate_liquidation(2, 7, &breached, 10_000).unwrap();
        assert_eq!(plan.close_qty, 100);
        assert!(plan.close_is_buy); // close short = buy
    }

    #[test]
    fn flat_account_never_liquidates() {
        let s = MarginState {
            equity: 0,
            position: Position::default(),
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        assert!(evaluate_liquidation(1, 7, &s, 10_000).is_none());
    }

    #[test]
    fn funding_pays_longs_when_premium_positive() {
        // premium +100bps on long 100 × mark 10_000 → long pays 100×10_000×1% = 10_000
        let pay = funding_payment(100, 300, 100, 10_000);
        assert_eq!(pay, -10_000);
        // Short receives.
        let recv = funding_payment(100, 300, -100, 10_000);
        assert_eq!(recv, 10_000);
    }

    #[test]
    fn funding_clamped_to_cap() {
        // premium 1000bps, cap 300bps → clamped to 300.
        // 300bps = 3% of 100 × 10_000 = 30_000.
        let pay = funding_payment(1_000, 300, 100, 10_000);
        assert_eq!(pay, -30_000);
        // Negative premium clamped to −cap → long receives 30_000.
        let recv = funding_payment(-1_000, 300, 100, 10_000);
        assert_eq!(recv, 30_000);
    }

    #[test]
    fn initial_requirement_higher_than_maintenance() {
        let mut p = Position::default();
        p.apply_fill(100, 10_000);
        let s = MarginState {
            equity: 1_000_000,
            position: p,
            maint_bps: 5_000,
            init_bps: 10_000,
        };
        // Initial (100% bps) = 100 × 10_000 × 1.0 = 1_000_000; maintenance 50%.
        assert_eq!(s.initial_requirement(10_000), 1_000_000);
        assert_eq!(s.maintenance_requirement(10_000), 500_000);
        assert!(s.initial_requirement(10_000) > s.maintenance_requirement(10_000));
    }
}
