-- Replace threshold-based clustering with HDBSCAN.
-- The threshold parameter is no longer needed; min_cluster_size
-- controls cluster granularity instead.

ALTER TABLE ontology_clusters DROP COLUMN threshold;
ALTER TABLE ontology_clusters ADD COLUMN min_cluster_size INT NOT NULL DEFAULT 2;
