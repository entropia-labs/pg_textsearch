/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * docmap.c - Document ID mapping implementation
 */
#include "docmap.h"
#include "fieldnorm.h"
#include "postgres.h"
#include "utils/hsearch.h"
#include "utils/memutils.h"

/* Initial capacity for document arrays */
#define DOCMAP_INITIAL_CAPACITY 1024

/*
 * Hash function for ItemPointerData keys
 */
static uint32
ctid_hash(const void *key, Size keysize)
{
	const ItemPointerData *ctid	  = (const ItemPointerData *)key;
	uint32				   block  = ItemPointerGetBlockNumber(ctid);
	uint16				   offset = ItemPointerGetOffsetNumber(ctid);

	(void)keysize; /* unused */

	/* Combine block and offset into a single hash */
	return block ^ ((uint32)offset << 16) ^ offset;
}

/*
 * Comparison function for ItemPointerData keys
 */
static int
ctid_match(const void *key1, const void *key2, Size keysize)
{
	(void)keysize; /* unused */
	return ItemPointerCompare((ItemPointer)key1, (ItemPointer)key2);
}

TpDocMapBuilder *
tp_docmap_create(void)
{
	TpDocMapBuilder *builder;
	HASHCTL			 hash_ctl;

	builder = palloc0(sizeof(TpDocMapBuilder));

	/* Create hash table for CTID → doc_id lookup */
	memset(&hash_ctl, 0, sizeof(hash_ctl));
	hash_ctl.keysize   = sizeof(ItemPointerData);
	hash_ctl.entrysize = sizeof(TpDocMapEntry);
	hash_ctl.hash	   = ctid_hash;
	hash_ctl.match	   = ctid_match;
	hash_ctl.hcxt	   = CurrentMemoryContext;

	builder->ctid_to_id = hash_create(
			"DocMap CTID->ID",
			DOCMAP_INITIAL_CAPACITY,
			&hash_ctl,
			HASH_ELEM | HASH_FUNCTION | HASH_COMPARE | HASH_CONTEXT);

	builder->num_docs		   = 0;
	builder->capacity		   = 0;
	builder->finalized		   = false;
	builder->ctid_pages		   = NULL;
	builder->ctid_offsets	   = NULL;
	builder->fieldnorms		   = NULL;
	builder->tenant_ids		   = NULL;
	builder->tenant_ranges	   = NULL;
	builder->num_tenant_ranges = 0;
	builder->tenant_ordered	   = false;

	return builder;
}

uint32
tp_docmap_add(
		TpDocMapBuilder *builder,
		ItemPointer		 ctid,
		uint32			 doc_length,
		uint32			 tenant_id)
{
	TpDocMapEntry *entry;
	bool		   found;

	Assert(!builder->finalized);

	/* Guard: UINT32_MAX is reserved as "not found" sentinel */
	if (builder->num_docs >= UINT32_MAX - 1)
		ereport(ERROR,
				(errcode(ERRCODE_PROGRAM_LIMIT_EXCEEDED),
				 errmsg("too many documents in segment (max %u)",
						UINT32_MAX - 1)));

	/* Look up or create entry in hash table */
	entry = (TpDocMapEntry *)
			hash_search(builder->ctid_to_id, ctid, HASH_ENTER, &found);

	if (found)
	{
		/* Document already exists, return existing ID */
		return entry->doc_id;
	}

	/* New document - assign next sequential ID */
	entry->doc_id	  = builder->num_docs;
	entry->doc_length = doc_length;
	entry->tenant_id  = tenant_id;
	builder->num_docs++;

	return entry->doc_id;
}

uint32
tp_docmap_lookup(TpDocMapBuilder *builder, ItemPointer ctid)
{
	TpDocMapEntry *entry;

	entry = (TpDocMapEntry *)
			hash_search(builder->ctid_to_id, ctid, HASH_FIND, NULL);

	if (entry == NULL)
		return UINT32_MAX;

	return entry->doc_id;
}

/*
 * Comparison function for sorting by CTID.
 * This ensures doc_ids are assigned in CTID order, which means
 * postings sorted by CTID are also sorted by doc_id - critical
 * for sequential access to CTID arrays during query iteration.
 */
static int
docmap_entry_cmp_by_ctid(const void *a, const void *b)
{
	const TpDocMapEntry *ea = (const TpDocMapEntry *)a;
	const TpDocMapEntry *eb = (const TpDocMapEntry *)b;

	/* Cast away const - ItemPointerCompare doesn't modify arguments */
	return ItemPointerCompare((ItemPointer)&ea->ctid, (ItemPointer)&eb->ctid);
}

/*
 * Comparison function for sorting by (tenant_id, CTID).
 * tenant_id=0 sorts first so non-tenant docs come before tenant docs.
 * Within a tenant, docs are sorted by CTID for locality.
 */
static int
docmap_entry_cmp_by_tenant_then_ctid(const void *a, const void *b)
{
	const TpDocMapEntry *ea = (const TpDocMapEntry *)a;
	const TpDocMapEntry *eb = (const TpDocMapEntry *)b;

	if (ea->tenant_id < eb->tenant_id)
		return -1;
	if (ea->tenant_id > eb->tenant_id)
		return 1;

	return ItemPointerCompare((ItemPointer)&ea->ctid, (ItemPointer)&eb->ctid);
}

void
tp_docmap_finalize(TpDocMapBuilder *builder)
{
	HASH_SEQ_STATUS scan;
	TpDocMapEntry  *entry;
	TpDocMapEntry  *entries;
	uint32			i;
	bool			has_tenants = false;

	Assert(!builder->finalized);

	if (builder->num_docs == 0)
	{
		builder->finalized = true;
		return;
	}

	/* Collect all entries from hash table */
	entries = palloc(sizeof(TpDocMapEntry) * builder->num_docs);
	i		= 0;

	hash_seq_init(&scan, builder->ctid_to_id);
	while ((entry = (TpDocMapEntry *)hash_seq_search(&scan)) != NULL)
	{
		entries[i] = *entry;
		if (entry->tenant_id != 0)
			has_tenants = true;
		i++;
	}

	Assert(i == builder->num_docs);

	/*
	 * Sort order depends on tenant data:
	 * - Non-tenant indexes: CTID order (original invariant)
	 * - Tenant indexes: (tenant_id, CTID) order so each tenant's
	 *   docs occupy a contiguous doc_id range, enabling O(K) BMW.
	 */
	if (has_tenants)
	{
		qsort(entries,
			  builder->num_docs,
			  sizeof(TpDocMapEntry),
			  docmap_entry_cmp_by_tenant_then_ctid);
		builder->tenant_ordered = true;
	}
	else
	{
		qsort(entries,
			  builder->num_docs,
			  sizeof(TpDocMapEntry),
			  docmap_entry_cmp_by_ctid);
		builder->tenant_ordered = false;
	}

	/* Allocate output arrays (split CTID storage for cache locality) */
	builder->capacity	  = builder->num_docs;
	builder->ctid_pages	  = palloc(sizeof(BlockNumber) * builder->num_docs);
	builder->ctid_offsets = palloc(sizeof(OffsetNumber) * builder->num_docs);
	builder->fieldnorms	  = palloc(sizeof(uint8) * builder->num_docs);

	/* Allocate tenant_ids array if any tenant data exists */
	if (has_tenants)
		builder->tenant_ids = palloc(sizeof(uint32) * builder->num_docs);

	/*
	 * Fill arrays and reassign doc_ids in sorted order.
	 * Update hash table entries so lookups return the correct doc_id.
	 */
	for (i = 0; i < builder->num_docs; i++)
	{
		TpDocMapEntry *hash_entry;

		builder->ctid_pages[i]	 = ItemPointerGetBlockNumber(&entries[i].ctid);
		builder->ctid_offsets[i] = ItemPointerGetOffsetNumber(
				&entries[i].ctid);
		builder->fieldnorms[i] = encode_fieldnorm(entries[i].doc_length);

		if (builder->tenant_ids)
			builder->tenant_ids[i] = entries[i].tenant_id;

		/* Update hash table entry with new doc_id */
		hash_entry = (TpDocMapEntry *)hash_search(
				builder->ctid_to_id, &entries[i].ctid, HASH_FIND, NULL);
		Assert(hash_entry != NULL);
		hash_entry->doc_id = i;
	}

	/*
	 * Compute tenant ranges by detecting boundaries in the sorted
	 * array. Each contiguous run of the same tenant_id becomes one
	 * TpDocMapTenantRange.
	 */
	if (has_tenants)
	{
		uint32				 range_cap = 16;
		uint32				 range_cnt = 0;
		TpDocMapTenantRange *ranges;
		uint32				 run_start	= 0;
		uint32				 run_tenant = entries[0].tenant_id;

		ranges = palloc(range_cap * sizeof(TpDocMapTenantRange));

		for (i = 1; i <= builder->num_docs; i++)
		{
			uint32 tid = (i < builder->num_docs) ? entries[i].tenant_id
												 : UINT32_MAX;

			if (tid != run_tenant)
			{
				/* Emit range for run_tenant (skip tenant_id=0) */
				if (run_tenant != 0)
				{
					if (range_cnt >= range_cap)
					{
						range_cap *= 2;
						ranges = repalloc(
								ranges,
								range_cap * sizeof(TpDocMapTenantRange));
					}
					ranges[range_cnt].tenant_id	   = run_tenant;
					ranges[range_cnt].first_doc_id = run_start;
					ranges[range_cnt].doc_count	   = i - run_start;
					range_cnt++;
				}
				run_start  = i;
				run_tenant = tid;
			}
		}

		builder->tenant_ranges	   = ranges;
		builder->num_tenant_ranges = range_cnt;
	}

	pfree(entries);
	builder->finalized = true;
}

void
tp_docmap_set_tenant_id(
		TpDocMapBuilder *builder, uint32 doc_id, uint32 tenant_id)
{
	Assert(builder->finalized);
	Assert(doc_id < builder->num_docs);

	/* Lazy-allocate tenant_ids array on first call */
	if (builder->tenant_ids == NULL)
		builder->tenant_ids = palloc0(builder->num_docs * sizeof(uint32));

	builder->tenant_ids[doc_id] = tenant_id;
}

void
tp_docmap_destroy(TpDocMapBuilder *builder)
{
	if (builder == NULL)
		return;

	if (builder->ctid_to_id)
		hash_destroy(builder->ctid_to_id);

	if (builder->ctid_pages)
		pfree(builder->ctid_pages);

	if (builder->ctid_offsets)
		pfree(builder->ctid_offsets);

	if (builder->fieldnorms)
		pfree(builder->fieldnorms);

	if (builder->tenant_ids)
		pfree(builder->tenant_ids);

	if (builder->tenant_ranges)
		pfree(builder->tenant_ranges);

	pfree(builder);
}
