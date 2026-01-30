# DeGate LP Handler Security Audit Requirements

**Document Version:** 1.0  
**Date:** January 31, 2026  
**Prepared For:** OtterSec  
**Prepared By:** DeGate Team

---

## 1. Executive Summary

DeGate is requesting a comprehensive security audit of the **LP Handler** Solana program. This smart contract implements a novel **"Entry Snapshot + Exit Reconciliation"** security model designed to ensure that no permission residuals or asset anomalies can occur during transaction execution.

The primary objective of this audit is to validate that the security architecture effectively constrains all asset flows to whitelisted accounts and prevents any unauthorized token transfers or approval escalations, regardless of how the underlying business logic is implemented.

---

## 2. Project Overview

### 2.1 Project Name
**DeGate LP Handler**

### 2.2 Blockchain
Solana

### 2.3 Programming Language
Rust (Anchor Framework)

### 2.4 Repository Information
- **Base Directory:** `programs/lp_handler/src/`
- **Commit Hash:** *[To be provided before audit start]*
- **Repository URL:** *[To be provided before audit start]*

### 2.5 Project Description
The LP Handler is a Solana program that manages liquidity pool operations with an emphasis on security through a unique architectural approach. Rather than relying solely on business logic correctness, the contract enforces security invariants at the entry and exit points of every transaction, creating a security boundary that business logic cannot bypass.

---

## 3. Security Architecture

### 3.1 Core Security Model: Entry Snapshot + Exit Reconciliation

The smart contract employs an **"Entry Snapshot + Exit Reconciliation"** security pattern that operates as follows:

1. **Entry Snapshot Phase:**
   - At transaction entry, the system captures a complete snapshot of all relevant account states
   - Records token balances, approval states, and account ownership for all accounts in scope
   - Establishes the baseline state against which exit conditions will be validated

2. **Exit Reconciliation Phase:**
   - Before transaction completion, the system performs comprehensive reconciliation
   - Validates that all asset movements comply with whitelist constraints
   - Ensures no unauthorized approvals have been created
   - Verifies LP NFT positions remain with user accounts without new delegations
   - Rejects the transaction if any security invariant is violated

3. **Security Guarantee:**
   - Business logic code in the `instructions/` module (out of audit scope) **CANNOT bypass** these security constraints regardless of implementation
   - **Prerequisite:** Third-party DAPP business logic is assumed to be non-malicious. The security model does NOT protect against malicious third-party DAPPs
   - Under this prerequisite, assets can ONLY flow to whitelisted accounts controlled by users or DeGate

---

## 4. Asset Flow Constraints

### 4.1 Whitelisted Account Categories

Assets are permitted to flow **ONLY** between the following four categories of whitelisted accounts:

| Category | Account Type | Description | Validation Rules |
|----------|--------------|-------------|------------------|
| **1.1** | `signer` | Transaction Caller | The account that initiates and signs the transaction |
| **1.2** | `recipient` | Asset Result Receiver | Defaults to same as signer; if different, wallet signing side performs strict validation: recipient MUST be a wallet-managed address (user has operational control) |
| **1.3** | `fee_owner` | DeGate Fee Account | DeGate's designated fee collection account |
| **1.4** | Third-party DAPP / Solana System Accounts | Business-specific Accounts | Read from on-chain data based on business characteristics; includes program-owned accounts and Solana system programs |

### 4.2 Asset Categories Under Protection

| Asset Type | Description | Flow Constraints |
|------------|-------------|------------------|
| **2.1** | LP Pool Token1 | Can ONLY flow between whitelisted accounts; token mint read from on-chain pool data |
| **2.2** | LP Pool Token2 | Can ONLY flow between whitelisted accounts; token mint read from on-chain pool data |
| **2.3** | User LP Position NFT (Token-2022) | MUST remain in user account; NO new approvals/delegations permitted |
| **2.4** | Reward Tokens | Can ONLY flow between whitelisted accounts; added based on business requirements |

### 4.3 Security Invariant

**Critical Prerequisite:** The security model operates under the assumption that third-party DAPP business logic is **non-malicious**. This security model does NOT aim to protect against malicious third-party DAPPs.

**Security Guarantee:** Under the prerequisite of non-malicious third-party DAPPs, all assets will ultimately reside ONLY in accounts controlled by:
- The user (signer / recipient)
- DeGate (fee_owner)

This invariant holds regardless of how the business logic in the `instructions/` module (out of audit scope) is implemented.

---

## 5. Audit Scope

### 5.1 Files IN SCOPE (Require Audit)

| File Path | Priority | Description |
|-----------|----------|-------------|
| `security/mod.rs` | **CRITICAL** | Core implementation of Entry Snapshot and Exit Reconciliation logic. This is the primary security boundary and requires the most thorough review. |
| `lib.rs` | **HIGH** | Program entry point; orchestrates the security model invocation and business logic flow. |
| `state/` | **MEDIUM** | Constants and state definitions; verify no security-relevant misconfigurations. |

### 5.2 Files OUT OF SCOPE (Skip Audit)

| File Path | Reason for Exclusion |
|-----------|---------------------|
| `instructions/` | Business logic code; the security model is designed to contain any vulnerabilities in this module |
| `test/` | Test code; not deployed to mainnet |

### 5.3 Audit Focus Areas

The audit should specifically verify:

1. **Snapshot Completeness**
   - Does the entry snapshot capture ALL relevant account states?
   - Are there any accounts or state variables that could be manipulated without detection?

2. **Reconciliation Correctness**
   - Does the exit reconciliation correctly identify ALL unauthorized asset movements?
   - Are there any edge cases where assets could leave whitelisted accounts undetected?

3. **Approval/Delegation Controls**
   - Is the LP NFT (Token-2022) approval checking comprehensive?
   - Can any new approvals be created that persist after transaction completion?

4. **Whitelist Integrity**
   - Can the whitelist be manipulated or bypassed?
   - Are the on-chain reads for third-party accounts secure against manipulation?

5. **Boundary Enforcement**
   - Can business logic in `instructions/` bypass the security checks in `security/mod.rs`?
   - Are there any code paths that skip the exit reconciliation?

6. **Token-2022 Specific Concerns**
   - Are Token-2022 extension features (transfer hooks, confidential transfers, etc.) handled correctly?
   - Can Token-2022 specific features be exploited to bypass security checks?

---

## 6. Known Considerations

### 6.1 Design Assumptions
- **Critical:** Third-party DAPP programs invoked via CPI are assumed to be non-malicious. The security model is NOT designed to defend against malicious third-party DAPPs
- The wallet signing interface enforces recipient validation (out of scope for this audit)
- On-chain pool data used for token mint discovery is trusted

### 6.2 Potential Risk Areas
- Reentrancy through CPI calls
- State manipulation between snapshot and reconciliation
- Integer overflow/underflow in balance calculations
- Account confusion or substitution attacks
- PDA derivation collisions

---

## 7. Deliverables Expected

1. **Comprehensive Audit Report** including:
   - Executive summary of findings
   - Detailed vulnerability descriptions with severity ratings (Critical/High/Medium/Low/Informational)
   - Proof of concept for any identified vulnerabilities
   - Remediation recommendations

2. **Specific Validation** of:
   - Whether the "Entry Snapshot + Exit Reconciliation" model achieves its stated security goals
   - Whether business logic can bypass the security boundary
   - Whether all asset flow constraints are properly enforced

3. **Code Quality Assessment** including:
   - Best practices compliance
   - Gas optimization opportunities
   - Documentation quality

---

## 8. Timeline and Communication

### 8.1 Preferred Timeline
- **Audit Duration:** *[To be discussed]*
- **Target Completion:** *[To be discussed]*

### 8.2 Communication Channels
- **Primary Contact:** *[To be provided]*
- **Technical Contact:** *[To be provided]*
- **Preferred Communication:** *[Slack/Discord/Email - To be specified]*

### 8.3 Availability for Questions
The DeGate development team will be available throughout the audit period to:
- Answer technical questions
- Clarify design intentions
- Discuss potential findings
- Review remediation approaches

---

## 9. Additional Information

### 9.1 Related Documentation
- *[Architecture diagrams - To be provided]*
- *[Technical specifications - To be provided]*
- *[Previous audit reports (if any) - To be provided]*

### 9.2 Test Environment
- Devnet deployment address: *[To be provided]*
- Test instructions: *[To be provided]*

---

## 10. Appendix: Security Model Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        TRANSACTION FLOW                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐                                                   │
│  │   ENTRY      │  ◄── Capture Snapshot:                            │
│  │   SNAPSHOT   │      • All token balances                         │
│  │              │      • All approval states                        │
│  │              │      • LP NFT ownership & delegations             │
│  └──────┬───────┘                                                   │
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────┐                                                   │
│  │   BUSINESS   │  ◄── Instructions Module (Out of Scope)           │
│  │   LOGIC      │      • LP operations                              │
│  │   EXECUTION  │      • Third-party CPI calls                      │
│  │              │      • Fee calculations                           │
│  └──────┬───────┘                                                   │
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────┐                                                   │
│  │   EXIT       │  ◄── Validate Against Snapshot:                   │
│  │   RECONCILE  │      ✓ Assets only in whitelist accounts          │
│  │              │      ✓ No new approvals on LP NFT                 │
│  │              │      ✓ LP NFT still with user                     │
│  │              │      ✗ REJECT if any violation                    │
│  └──────────────┘                                                   │
│                                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                     WHITELIST ACCOUNTS                               │
│  ┌─────────┐  ┌───────────┐  ┌───────────┐  ┌──────────────────┐   │
│  │ Signer  │  │ Recipient │  │ Fee Owner │  │ System/DAPP Accts│   │
│  │  (1.1)  │  │   (1.2)   │  │   (1.3)   │  │      (1.4)       │   │
│  └─────────┘  └───────────┘  └───────────┘  └──────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

---

**Document End**

*This document is confidential and intended solely for the use of OtterSec for the purpose of conducting a security audit of the DeGate LP Handler program.*
