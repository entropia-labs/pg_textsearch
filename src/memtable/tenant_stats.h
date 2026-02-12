/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * tenant_stats.h - Per-tenant corpus statistics in shared memory
 *
 * Maintains per-tenant document count and total token length
 * in a dshash table for accurate per-tenant BM25 scoring.
 */
#pragma once

#include <postgres.h>

#include <lib/dshash.h>
#include <utils/dsa.h>

#include "state/state.h"

/*
 * Per-tenant statistics entry in the dshash table.
 * Key is tenant_id (first field).
 */
typedef struct TpTenantStatsEntry
{
	uint32 tenant_id; /* Hash key */
	uint32 doc_count; /* Number of documents for this tenant */
	int64  total_len; /* Sum of document lengths for this tenant */
} TpTenantStatsEntry;

/* Tranche ID for tenant stats dshash */
#define TP_TRANCHE_TENANT_STATS 1009

/*
 * Update per-tenant statistics after inserting a document.
 * Finds or creates an entry for the given tenant_id and
 * increments doc_count by 1 and total_len by doc_length.
 */
extern void tp_update_tenant_stats(
		TpLocalIndexState *local_state, uint32 tenant_id, int32 doc_length);

/*
 * Look up per-tenant statistics.
 * Returns true if the tenant was found, false otherwise.
 * When found, sets *doc_count and *total_len.
 */
extern bool tp_get_tenant_stats(
		TpLocalIndexState *local_state,
		uint32			   tenant_id,
		uint32			  *doc_count,
		int64			  *total_len);

/*
 * Create the tenant stats dshash table in the given DSA area.
 * Returns the handle for storage in TpMemtable.
 */
extern dshash_table_handle tp_tenant_stats_create(dsa_area *area);

/*
 * Attach to an existing tenant stats dshash table.
 */
extern dshash_table *
tp_tenant_stats_attach(dsa_area *area, dshash_table_handle handle);

/*
 * Check if the tenant stats infrastructure is initialized
 * (i.e., the dshash table exists in shared memory). Returns
 * true if stats are available, false for pre-tenant indexes.
 */
extern bool tp_tenant_stats_initialized(TpLocalIndexState *local_state);

/*
 * Iterate all tenant stats entries and collect them into an
 * array. Returns the number of entries collected. The caller
 * must pfree the returned array.
 */
extern int tp_tenant_stats_collect(
		TpLocalIndexState *local_state, TpTenantStatsEntry **entries_out);
