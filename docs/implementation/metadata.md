# Metadata and how it is stored

## Mapping subgraph names to deployments

### `subgraphs.subgraph`

List of all known subgraph names. Maintained in the primary, but there is a background job that periodically copies the table from the primary to all other shards. Those copies are used for queries when the primary is down.

| Column            | Type         | Use                                       |
| ----------------- | ------------ | ----------------------------------------- |
| `id`              | `text!`      | primary key, UUID                         |
| `name`            | `text!`      | user-chosen name                          |
| `current_version` | `text`       | `subgraph_version.id` for current version |
| `pending_version` | `text`       | `subgraph_version.id` for pending version |
| `created_at`      | `numeric!`   | UNIX timestamp                            |
| `vid`             | `int8!`      | unused                                    |
| `block_range`     | `int4range!` | unused                                    |

The `id` is used by the hosted explorer to reference the subgraph.

### `subgraphs.subgraph_version`

Mapping of subgraph names from `subgraph` to IPFS hashes. Maintained in the primary, but there is a background job that periodically copies the table from the primary to all other shards. Those copies are used for queries when the primary is down.

| Column        | Type         | Use                     |
| ------------- | ------------ | ----------------------- |
| `id`          | `text!`      | primary key, UUID       |
| `subgraph`    | `text!`      | `subgraph.id`           |
| `deployment`  | `text!`      | IPFS hash of deployment |
| `created_at`  | `numeric`    | UNIX timestamp          |
| `vid`         | `int8!`      | unused                  |
| `block_range` | `int4range!` | unused                  |

## Managing a deployment

Directory of all deployments. Maintained in the primary, but there is a background job that periodically copies the table from the primary to all other shards. Those copies are used for queries when the primary is down.

### `public.deployment_schemas`

| Column       | Type           | Use                                          |
| ------------ | -------------- | -------------------------------------------- |
| `id`         | `int4!`        | primary key                                  |
| `subgraph`   | `text!`        | IPFS hash of deployment                      |
| `name`       | `text!`        | name of `sgdNNN` schema                      |
| `version`    | `int4!`        | version of data layout in `sgdNNN`           |
| `shard`      | `text!`        | database shard holding data                  |
| `network`    | `text!`        | network/chain used                           |
| `active`     | `boolean!`     | whether to query this copy of the deployment |
| `created_at` | `timestamptz!` |                                              |

There can be multiple copies of the same deployment, but at most one per shard. The `active` flag indicates which of these copies will be used for queries; `graph-node` makes sure that there is always exactly one for each IPFS hash.

### `subgraphs.head`

Details about a deployment that change on every block. Maintained in the
shard alongside the deployment's data in `sgdNNN`.

| Column            | Type       | Use                                          |
| ----------------- | ---------- | -------------------------------------------- |
| `id`              | `integer!` | primary key, same as `deployment_schemas.id` |
| `block_hash`      | `bytea`    | current subgraph head                        |
| `block_number`    | `numeric`  |                                              |
| `entity_count`    | `numeric!` | total number of entities                     |
| `firehose_cursor` | `text`     |                                              |

The head block pointer in `block_number` and `block_hash` is the latest
block that has been fully processed by the deployment. It will be `null`
until the deployment is fully initialized, and only set when the deployment
processes the first block. For deployments that are grafted or being copied,
the head block pointer will be `null` until the graft/copy has finished
which can take considerable time.

### `subgraphs.deployment`

Details about a deployment to track sync progress etc. Maintained in the
shard alongside the deployment's data in `sgdNNN`. The table should only
contain data that changes fairly infrequently, but for historical reasons
contains also static data.

| Column                               | Type          | Use                                                  |
| ------------------------------------ | ------------- | ---------------------------------------------------- |
| `id`                                 | `integer!`    | primary key, same as `deployment_schemas.id`         |
| `subgraph`                           | `text!`       | IPFS hash                                            |
| `earliest_block_number`              | `integer!`    | earliest block for which we have data                |
| `health`                             | `health!`     |                                                      |
| `failed`                             | `boolean!`    |                                                      |
| `fatal_error`                        | `text`        |                                                      |
| `non_fatal_errors`                   | `text[]`      |                                                      |
| `graft_base`                         | `text`        | IPFS hash of graft base                              |
| `graft_block_hash`                   | `bytea`       | graft block                                          |
| `graft_block_number`                 | `numeric`     |                                                      |
| `reorg_count`                        | `integer!`    |                                                      |
| `current_reorg_depth`                | `integer!`    |                                                      |
| `max_reorg_depth`                    | `integer!`    |                                                      |
| `last_healthy_ethereum_block_hash`   | `bytea`       |                                                      |
| `last_healthy_ethereum_block_number` | `numeric`     |                                                      |
| `debug_fork`                         | `text`        |                                                      |
| `synced_at`                          | `timestamptz` | time when deployment first reach chain head          |
| `synced_at_block_number`             | `integer`     | block number where deployment first reach chain head |

The columns `reorg_count`, `current_reorg_depth`, and `max_reorg_depth` are
set during indexing. They are used to determine whether a reorg happened
while a query was running, and whether that reorg could have affected the
query.

### `subgraphs.subgraph_manifest`

Details about a deployment that rarely change. Maintained in the
shard alongside the deployment's data in `sgdNNN`.

| Column                  | Type       | Use                                                  |
| ----------------------- | ---------- | ---------------------------------------------------- |
| `id`                    | `integer!` | primary key, same as `deployment_schemas.id`         |
| `spec_version`          | `text!`    |                                                      |
| `description`           | `text`     |                                                      |
| `repository`            | `text`     |                                                      |
| `schema`                | `text!`    | GraphQL schema                                       |
| `features`              | `text[]!`  |                                                      |
| `graph_node_version_id` | `integer`  |                                                      |
| `use_bytea_prefix`      | `boolean!` |                                                      |
| `start_block_hash`      | `bytea`    | Parent of the smallest start block from the manifest |
| `start_block_number`    | `int4`     |                                                      |
| `on_sync`               | `text`     | Additional behavior when deployment becomes synced   |
| `history_blocks`        | `int4!`    | How many blocks of history to keep                   |

### `subgraphs.subgraph_deployment_assignment`

Tracks which index node is indexing a deployment. Maintained in the primary,
but there is a background job that periodically copies the table from the
primary to all other shards.

| Column  | Type  | Use                                         |
| ------- | ----- | ------------------------------------------- |
| id      | int4! | primary key, ref to `deployment_schemas.id` |
| node_id | text! | name of index node                          |

This table could simply be a column on `deployment_schemas`.

### `subgraphs.dynamic_ethereum_contract_data_source`

Stores the dynamic data sources for all subgraphs (will be turned into a
table that lives in each subgraph's namespace `sgdNNN` soon)

### `subgraphs.subgraph_error`

Stores details about errors that subgraphs encounter during indexing.

### Copying of deployments

The tables `active_copies` in the primary, and `subgraphs.copy_state` and
`subgraphs.copy_table_state` are used to track which deployments need to be
copied and how far copying has progressed to make sure that copying works
correctly across index node restarts.

### Influencing query generation

The table `subgraphs.table_stats` stores which tables for a deployment
should have the 'account-like' optimization turned on.

### `subgraphs.subgraph_features`

Details about features that a deployment uses, Maintained in the primary.

| Column         | Type      | Use         |
| -------------- | --------- | ----------- |
| `id`           | `text!`   | primary key |
| `spec_version` | `text!`   |             |
| `api_version`  | `text`    |             |
| `features`     | `text[]!` |             |
| `data_sources` | `text[]!` |             |
| `handlers`     | `text[]!` |             |
