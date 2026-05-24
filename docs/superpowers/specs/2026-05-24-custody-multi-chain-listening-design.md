# Custody Multi-Chain Listening Design

## Goal

Upgrade custody accounts so one custody account represents one blockchain address text with multiple chain listening configurations. Each chain configuration selects one or more assets. When a custody account is assigned, the system automatically creates or activates watched addresses for every configured chain and binds the configured assets.

## Current Context

The existing custody implementation stores `chain_id` directly on `custody_accounts`, so each custody account is tied to one chain. Assignment currently ensures one watched address and only binds the chain native asset.

Relevant existing files:

- `backend/crates/storage/migrations/0020_custody_accounts.sql`
- `backend/crates/storage/src/custody_accounts.rs`
- `backend/crates/core/src/models.rs`
- `backend/crates/api-server/src/routes.rs`
- `frontend/src/pages/CustodyAccountsPage.tsx`
- `frontend/src/pages/AddressesPage.tsx`

The existing watched-address model already supports multiple selected assets through `watched_address_assets`. The address import UI already has a reusable multi-chain, multi-asset pattern.

## Product Rules

1. One custody account represents one address text, not one chain/address pair.
2. A custody account has one or more chain configurations.
3. Each chain configuration has one chain and one or more assets from that chain.
4. Assigning a custody account uses the account's preconfigured chain and asset set. The first version does not allow assignment-time overrides.
5. Assignment is transactional: all configured watched addresses and asset bindings are applied, or nothing is committed.
6. Releasing an assignment releases custody ownership but does not delete or deactivate watched addresses.
7. System pool accounts are selected from available custody accounts. User-provided accounts are matched or created by normalized address.
8. The same tenant cannot have two custody accounts for the same normalized address.
9. The same custody account cannot have more than one active assignment.
10. The same tenant and business reference cannot be assigned twice.
11. User-provided assignment may create a new custody account; in that case, the request must provide the new account's chain configs. Existing user accounts keep their saved configs and are not overridden during assignment.

## Data Model

### `custody_accounts`

Keep the table as the parent custody object, but treat `address` and `address_normalized` as address-level fields. The existing `chain_id` column becomes compatibility data during the transition and should no longer be the source of truth after chain configs are introduced.

Recommended compatibility behavior:

- Keep `chain_id` initially to avoid a large destructive migration.
- Add chain config rows for every existing custody account using the existing `chain_id`.
- New code reads chain configs and only uses `custody_accounts.chain_id` as a fallback during migration safety checks.
- Change custody account address uniqueness from `(tenant_id, chain_id, address_normalized)` to `(tenant_id, address_normalized)`, because the account is now address-level instead of chain-level.

### `custody_account_chain_configs`

Create a new table:

```sql
CREATE TABLE custody_account_chain_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    custody_account_id UUID NOT NULL,
    chain_id UUID NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, custody_account_id, chain_id),
    FOREIGN KEY (custody_account_id, tenant_id)
        REFERENCES custody_accounts(id, tenant_id)
        ON DELETE CASCADE
);
```

### `custody_account_chain_config_assets`

Create a normalized asset mapping table:

```sql
CREATE TABLE custody_account_chain_config_assets (
    chain_config_id UUID NOT NULL REFERENCES custody_account_chain_configs(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY (chain_config_id, asset_id)
);
```

Asset-chain consistency is validated in application code by checking each selected asset belongs to the config chain and is active. This matches existing `watched_address_assets` validation patterns.

## API Contract

### `CreateCustodyAccountRequest`

Add `chain_configs`:

```ts
type CustodyAccountChainConfigRequest = {
  chain_id: string;
  asset_ids: string[];
};

type CreateCustodyAccountRequest = {
  address: string;
  label?: string | null;
  source: 'pool' | 'user';
  status?: 'available' | 'assigned' | 'disabled';
  chain_configs: CustodyAccountChainConfigRequest[];
};

type AssignCustodyAccountRequest = {
  source: 'pool' | 'user';
  address?: string | null;
  applicant_type: 'api' | 'internal';
  business_ref: string;
  purpose?: string | null;
  chain_configs?: CustodyAccountChainConfigRequest[];
};
```

For backward compatibility during implementation, `chain_id` can remain temporarily in Rust/TypeScript DTOs, but new create flows must send `chain_configs` and storage must require at least one config. Pool assignment does not accept `chain_configs`; user assignment requires `chain_configs` only when the normalized address has no existing custody account.

### `CustodyAccount`

Add chain config details:

```ts
type CustodyAccountChainConfig = {
  id: string;
  chain_id: string;
  chain_name: string;
  asset_ids: string[];
  asset_symbols: string[];
};

type CustodyAccount = {
  id: string;
  address: string;
  label?: string | null;
  source: string;
  status: string;
  chain_configs: CustodyAccountChainConfig[];
};
```

### `AssignCustodyAccountResponse`

Return all watched addresses touched by the assignment:

```ts
type CustodyAssignmentWatchedAddress = {
  chain_id: string;
  chain_name: string;
  watched_address_id: string;
  asset_ids: string[];
};

type AssignCustodyAccountResponse = {
  account: CustodyAccount;
  assignment: CustodyAccountAssignment;
  watched_addresses: CustodyAssignmentWatchedAddress[];
};
```

The assignment row may keep a nullable legacy `watched_address_id`, but the response must not imply there is only one watched address.

## Storage Behavior

### Creating a custody account

1. Trim the address.
2. Require at least one chain config.
3. Require unique `chain_id` values inside the request.
4. For every chain config:
   - Load the chain.
   - Validate the address for that chain.
   - Require at least one asset.
   - Validate all assets are active and belong to that chain.
5. Derive account-level `address_normalized` by trimming and lowercasing only when any configured chain is EVM. If no configured chain is EVM, preserve trimmed casing. This supports shared EVM addresses such as Ethereum/Base while avoiding case loss for non-EVM addresses.
6. Insert the custody account.
7. Insert chain config rows and asset mapping rows.
8. Return the account with expanded config details.

### Assigning a custody account

1. Lock and select the custody account using existing pool/user rules.
2. Validate it has at least one chain config.
3. Validate no active assignment exists.
4. For each chain config:
   - Validate the custody address for that chain.
   - Create or reactivate the watched address for that chain and tenant.
   - Ensure every configured asset is present in `watched_address_assets`.
5. Insert one custody assignment.
6. Commit the transaction.
7. Return account, assignment, and all watched addresses touched.

If any chain config fails, rollback the full transaction.

### Release behavior

Release remains assignment-level:

- Active assignment becomes `released`.
- Pool custody account becomes `available`.
- User custody account remains non-pool and is not made pool-available.
- Watched addresses and watched address asset mappings remain active.

## Frontend Behavior

### Create custody account modal

Update the modal to configure:

- Address
- Label
- Chain config rows

Each row contains:

- Chain select
- Multi-select assets filtered by selected chain
- Remove row action

Rules:

- At least one chain row is required.
- Chain rows cannot repeat the same chain.
- Every row must select at least one asset.
- Asset options only include active assets for that row's chain.
- The create modal remains pool-only in the first version: `source='pool'`, `status='available'`.

Reuse the multi-chain and multi-asset UI pattern from `frontend/src/pages/AddressesPage.tsx`.

### Assignment modal

The assignment modal does not choose assets. It explains that the selected custody account's configured chains and assets are applied automatically.

For `source=user`, the user address still must be provided. The assignment form includes the same chain config rows as create. If the normalized user address does not exist, those rows create the new user custody account config. If the normalized user address already exists, the saved account config is used and the submitted config is ignored. The UI must explain this behavior to avoid surprises.

### Tables

Add columns or summaries for:

- Configured chains
- Asset symbols per chain
- Watched address count from the last assignment response if available

Keep table content compact with ellipsis or tooltip patterns already used in the app.

## Validation and Error Handling

Return validation errors for:

- Empty `chain_configs`.
- Duplicate chain configs.
- Empty asset list for a chain.
- Asset not found, inactive, or not in the selected chain.
- Address invalid for any configured chain.
- Custody account with no chain configs at assignment time.
- Duplicate active assignment.
- Duplicate business reference.

Do not partially apply watched-address changes on validation failure.

## Testing Strategy

### Backend tests

Add focused source/unit tests covering:

- Migration creates chain config and asset mapping tables.
- DTOs serialize and deserialize `chain_configs` and assignment watched-address response.
- Storage validation rejects empty configs, duplicate chains, empty assets, and cross-chain assets.
- Assignment query/path ensures watched addresses for all configured chains.
- Assignment conflict handling still returns validation errors for duplicate active assignment and duplicate business reference.
- Release keeps watched addresses active.

### Frontend tests

Extend `frontend/src/ui-regression.test.ts` to assert:

- Custody types expose `chain_configs`.
- Custody page has multi-chain rows.
- Asset select is multiple and chain-filtered.
- Duplicate chain and empty asset validation text exists.
- Assignment response handles multiple watched addresses.

### Full verification

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
cargo test --manifest-path backend/Cargo.toml -p coin-listener-core custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p coin-listener-storage custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml -p api-server custody -- --nocapture
cargo test --manifest-path backend/Cargo.toml --quiet
npm --prefix frontend run test:ui-regression
npm --prefix frontend run build
```

## Non-Goals

This design does not add:

- Private-key custody.
- Signing.
- Withdrawals.
- Address generation.
- Assignment-time override for existing account configs.
- Automatic deletion or deactivation of watched addresses on release.
