/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * tenant_stats.c - Per-tenant corpus statistics in shared memory
 *
 * Uses a dshash table to maintain per-tenant document counts and
 * total token lengths for accurate per-tenant BM25 scoring.
 */
#include <postgres.h>

#include <lib/dshash.h>
#include <utils/dsa.h>

#include "memtable/tenant_stats.h"
#include "state/state.h"

/*
 * dshash parameters for the tenant stats table.
 * Key is uint32 tenant_id at offset 0.
 */
static const dshash_parameters tenant_stats_params = {
		.key_size		  = sizeof(uint32),
		.entry_size		  = sizeof(TpTenantStatsEntry),
		.compare_function = dshash_memcmp,
		.hash_function	  = dshash_memhash,
		.copy_function	  = dshash_memcpy,
		.tranche_id		  = TP_TRANCHE_TENANT_STATS,
};

/*
 * Create a new tenant stats dshash table.
 */
dshash_table_handle
tp_tenant_stats_create(dsa_area *area)
{
	dshash_table	   *table;
	dshash_table_handle handle;

	table  = dshash_create(area, &tenant_stats_params, NULL);
	handle = dshash_get_hash_table_handle(table);
	dshash_detach(table);

	return handle;
}

/*
 * Attach to an existing tenant stats dshash table.
 */
dshash_table *
tp_tenant_stats_attach(dsa_area *area, dshash_table_handle handle)
{
	return dshash_attach(area, &tenant_stats_params, handle, NULL);
}

/*
 * Update per-tenant statistics after inserting a document.
 */
void
tp_update_tenant_stats(
		TpLocalIndexState *local_state, uint32 tenant_id, int32 doc_length)
{
	TpMemtable		   *memtable;
	dshash_table	   *table;
	TpTenantStatsEntry *entry;
	bool				found;

	Assert(local_state != NULL);
	Assert(tenant_id != 0);

	memtable = get_memtable(local_state);
	if (!memtable)
		return;

	/* Create tenant stats table if it doesn't exist */
	if (memtable->tenant_stats_handle == DSHASH_HANDLE_INVALID)
	{
		memtable->tenant_stats_handle = tp_tenant_stats_create(
				local_state->dsa);
	}

	table = tp_tenant_stats_attach(
			local_state->dsa, memtable->tenant_stats_handle);
	if (!table)
		return;

	/* Find or insert entry for this tenant */
	entry = (TpTenantStatsEntry *)
			dshash_find_or_insert(table, &tenant_id, &found);

	if (!found)
	{
		entry->tenant_id = tenant_id;
		entry->doc_count = 1;
		entry->total_len = doc_length;
	}
	else
	{
		entry->doc_count++;
		entry->total_len += doc_length;
	}

	dshash_release_lock(table, entry);
	dshash_detach(table);
}

/*
 * Look up per-tenant statistics.
 */
bool
tp_get_tenant_stats(
		TpLocalIndexState *local_state,
		uint32			   tenant_id,
		uint32			  *doc_count,
		int64			  *total_len)
{
	TpMemtable		   *memtable;
	dshash_table	   *table;
	TpTenantStatsEntry *entry;

	Assert(local_state != NULL);
	Assert(tenant_id != 0);

	memtable = get_memtable(local_state);
	if (!memtable)
		return false;

	if (memtable->tenant_stats_handle == DSHASH_HANDLE_INVALID)
		return false;

	table = tp_tenant_stats_attach(
			local_state->dsa, memtable->tenant_stats_handle);
	if (!table)
		return false;

	entry = (TpTenantStatsEntry *)dshash_find(table, &tenant_id, false);

	if (entry)
	{
		*doc_count = entry->doc_count;
		*total_len = entry->total_len;
		dshash_release_lock(table, entry);
		dshash_detach(table);
		return true;
	}

	dshash_detach(table);
	return false;
}

/*
 * Collect all tenant stats entries into a palloc'd array.
 * Returns the number of entries. Caller must pfree.
 */
int
tp_tenant_stats_collect(
		TpLocalIndexState *local_state, TpTenantStatsEntry **entries_out)
{
	TpMemtable		   *memtable;
	dshash_table	   *table;
	dshash_seq_status	status;
	TpTenantStatsEntry *entry;
	TpTenantStatsEntry *result;
	int					count	 = 0;
	int					capacity = 32;

	*entries_out = NULL;

	memtable = get_memtable(local_state);
	if (!memtable || memtable->tenant_stats_handle == DSHASH_HANDLE_INVALID)
		return 0;

	table = tp_tenant_stats_attach(
			local_state->dsa, memtable->tenant_stats_handle);
	if (!table)
		return 0;

	result = palloc(capacity * sizeof(TpTenantStatsEntry));

	dshash_seq_init(&status, table, false);
	while ((entry = (TpTenantStatsEntry *)dshash_seq_next(&status)) != NULL)
	{
		if (count >= capacity)
		{
			capacity *= 2;
			result = repalloc(result, capacity * sizeof(TpTenantStatsEntry));
		}
		result[count++] = *entry;
	}
	dshash_seq_term(&status);
	dshash_detach(table);

	*entries_out = result;
	return count;
}
