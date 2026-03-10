-- Ontology clustering: store discovered clusters and canonical labels.
--
-- Preserves original node_type and rel_type for provenance while
-- adding canonical_* columns that reflect the clustered ontology.

-- Cluster definitions.
CREATE TABLE ontology_clusters (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    level TEXT NOT NULL,               -- 'entity', 'entity_type', 'rel_type'
    canonical_label TEXT NOT NULL,
    member_labels JSONB NOT NULL DEFAULT '[]',
    member_count INT NOT NULL DEFAULT 0,
    threshold FLOAT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_ontology_clusters_level ON ontology_clusters(level);
CREATE INDEX idx_ontology_clusters_canonical ON ontology_clusters(canonical_label);

-- Canonical type column on nodes (preserves original node_type).
ALTER TABLE nodes ADD COLUMN canonical_type TEXT;

-- Link nodes to their entity-name cluster.
ALTER TABLE nodes ADD COLUMN cluster_id UUID REFERENCES ontology_clusters(id);

-- Canonical relationship type on edges (preserves original rel_type).
ALTER TABLE edges ADD COLUMN canonical_rel_type TEXT;
