-- Phase 4: Graph Embeddings storage (covalence#51)
-- Node2Vec and Spectral embeddings per node.

CREATE TABLE IF NOT EXISTS covalence.graph_embeddings (
    node_id    UUID        NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    method     TEXT        NOT NULL CHECK (method IN ('node2vec', 'spectral')),
    embedding  vector(64)  NOT NULL,
    computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (node_id, method)
);

CREATE INDEX IF NOT EXISTS idx_graph_embeddings_node2vec
    ON covalence.graph_embeddings USING hnsw (embedding vector_cosine_ops)
    WHERE method = 'node2vec';

CREATE INDEX IF NOT EXISTS idx_graph_embeddings_spectral
    ON covalence.graph_embeddings USING hnsw (embedding vector_cosine_ops)
    WHERE method = 'spectral';
