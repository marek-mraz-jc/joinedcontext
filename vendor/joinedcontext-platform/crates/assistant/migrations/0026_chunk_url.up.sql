-- T-7765: the lookups of a page by its address (held_page_projects, dataset_reads_projects,
-- titles) matched `c.url` with no index, so each read the project's passages from the heap:
-- on prod 112k chunks, 1.1 GB, and the narrowing waited out its 2 s in 11 of 14 questions.
CREATE INDEX chunks_url ON chunks (project, url);
