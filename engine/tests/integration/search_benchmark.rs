//! Search quality benchmark — covalence#24.
//!
//! Measures Precision@1, Precision@3, and Precision@5 across a standard
//! 11-query suite spanning 10 knowledge domains.  Latency is recorded per
//! query and every benchmark asserts that aggregate metrics remain above the
//! established baseline thresholds.
//!
//! # Design
//!
//! * **Fixture**: 20 nodes (10 articles + 10 sources) are inserted with
//!   highly-distinctive, domain-specific terminology.  Each topic cluster
//!   shares one anchor term so that a single query can retrieve both the
//!   article and its backing source.
//! * **Lexical-only**: no embedding is supplied in the [`SearchRequest`] so
//!   the vector dimension is skipped and only `ts_rank` + graph contribute.
//!   This makes results fully deterministic without a live embedding service.
//! * **OR-split queries**: each query is written as
//!   `"article-specific terms" OR "source-specific terms"` using
//!   `websearch_to_tsquery` boolean OR syntax.  This ensures both the article
//!   *and* the source for a topic satisfy the query while non-topic documents
//!   are excluded.  Note: `websearch_to_tsquery` treats a bare hyphen as a
//!   NOT operator, so terms like `SHA-256` are intentionally avoided in favour
//!   of their hyphen-free equivalents.
//!
//! # Thresholds (baseline)
//!
//! | Metric        | Threshold |
//! |---------------|-----------|
//! | Mean P@1      | ≥ 0.80    |
//! | Mean P@3      | ≥ 0.55    |
//! | Mean P@5      | ≥ 0.35    |
//! | Latency (p99) | < 2 000ms |
//!
//! Run:
//! ```bash
//! cargo test --test integration search_benchmark -- --test-threads=1
//! ```

use std::collections::HashSet;
use std::time::Instant;

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── types ────────────────────────────────────────────────────────────────────

/// A single benchmark query paired with the node IDs that are considered
/// relevant for it.
struct BenchmarkCase {
    /// Human-readable label (used in diagnostic output).
    label: &'static str,
    /// The query string exactly as it would be sent by a client.
    /// Uses `websearch_to_tsquery` OR-split syntax so both article and source
    /// for a topic satisfy the query.
    query: &'static str,
    /// Node IDs that should appear in the top results.
    /// Populated at fixture-creation time.
    relevant_ids: Vec<Uuid>,
}

/// Per-query precision and timing results.
#[derive(Debug)]
struct CaseResult {
    label: &'static str,
    p_at_1: f64,
    p_at_3: f64,
    p_at_5: f64,
    latency_ms: u64,
}

// ─── fixture builder ──────────────────────────────────────────────────────────

/// Insert all 20 benchmark nodes and return the 11 [`BenchmarkCase`]s with
/// their `relevant_ids` filled in.
///
/// # Content strategy
///
/// Each topic gets one article and one source.  Both share an *anchor* term
/// so a query can find either node, but each document also carries several
/// *distinctive* terms that appear nowhere else in the fixture — this is what
/// gives `ts_rank` clean signal and keeps inter-topic precision high.
///
/// # Hyphen caution
///
/// PostgreSQL's `websearch_to_tsquery` treats a bare `-` as NOT.  Content is
/// written with hyphen-free spellings where that would interfere (e.g. "btree"
/// rather than "B-tree", "sha256" rather than "SHA-256").
async fn build_fixture(fix: &mut TestFixture) -> Vec<BenchmarkCase> {
    // ── Topic 1: Machine Learning ─────────────────────────────────────────────
    // Anchor term shared by both docs: "backpropagation"
    // Article-distinctive: gradient, descent, minibatch, convergence
    // Source-distinctive : dropout, regularization, relu, activation
    let ml_article = fix
        .insert_article(
            "Gradient Descent and Backpropagation",
            "Gradient descent is the foundational optimization algorithm used to train \
             neural networks through backpropagation of error signals.  By computing the \
             gradient of the loss function with respect to each weight and stepping in the \
             negative gradient direction, the network learns iteratively.  Minibatch \
             stochastic gradient descent balances computational efficiency with convergence \
             stability.  Key hyperparameters include the learning rate, momentum, and the \
             number of training epochs.  Adaptive optimizers such as Adam and RMSprop \
             adjust the effective learning rate per parameter using running estimates of \
             gradient moments, accelerating convergence on sparse-gradient problems.",
        )
        .await;

    let ml_source = fix
        .insert_source(
            "Regularization Techniques for Deep Networks",
            "Backpropagation enables the computation of gradients through deep neural \
             architectures by applying the chain rule layer by layer.  Dropout \
             regularization randomly deactivates neurons during each forward pass, acting \
             as an ensemble method over exponentially many sub-networks.  L2 weight decay \
             applies a Gaussian prior on weights, penalising large magnitude activations.  \
             Batch normalisation smooths the loss landscape and reduces sensitivity to the \
             initial learning rate.  ReLU activation functions alleviate the vanishing \
             gradient problem that plagued earlier sigmoid-based architectures.  The relu \
             activation function has become the standard default in modern deep learning.",
        )
        .await;

    // ── Topic 2: Distributed Systems ─────────────────────────────────────────
    // Anchor: "consensus"
    // Article-distinctive: Raft, leader, election, quorum, replication
    // Source-distinctive : Kafka, partition, broker, consumer, offset
    let ds_article = fix
        .insert_article(
            "Raft Consensus Algorithm",
            "Raft is a consensus algorithm engineered for understandability in distributed \
             systems.  A single leader is elected per term via randomised election timeouts; \
             the leader accepts client writes, appends them to its log, and replicates \
             entries to a quorum of followers before committing.  Leader failure triggers a \
             new election among up-to-date followers.  Raft guarantees linearisability: \
             once a log entry is committed it is durable across failures.  Split-brain is \
             prevented because only a node with a majority quorum can elect a new leader \
             and commit consensus entries.",
        )
        .await;

    let ds_source = fix
        .insert_source(
            "Apache Kafka Distributed Log Architecture",
            "Kafka achieves high-throughput durable messaging by modelling topics as \
             partitioned, replicated logs.  Producers append records to partition leaders; \
             followers replicate asynchronously.  Consumer groups track per-partition \
             offsets, enabling independent replay.  The group coordinator manages broker \
             membership and triggers rebalancing when consumers join or leave.  Kafka's \
             design explicitly separates storage from compute, allowing brokers to serve \
             as a durable distributed log without in-memory state.  Each Kafka broker \
             stores a subset of partitions and handles consumer fetch requests.",
        )
        .await;

    // ── Topic 3: Cryptography ─────────────────────────────────────────────────
    // Anchor: "cryptographic"
    // Article-distinctive: hash, preimage, collision, merkle, digest
    //   NOTE: "sha256" used (not "SHA-256") to avoid websearch_to_tsquery
    //   treating the hyphen as a NOT operator.
    // Source-distinctive : RSA, asymmetric, modular, exponentiation, cipher
    let crypto_article = fix
        .insert_article(
            "SHA256 and Cryptographic Hash Properties",
            "SHA256 is a member of the SHA2 family of cryptographic hash functions \
             producing a 256-bit digest.  A secure cryptographic hash must satisfy \
             preimage resistance (infeasibility of inverting the digest), \
             second-preimage resistance, and collision resistance.  The \
             Merkle-Damgard construction processes input blocks in sequence, mixing \
             through compression rounds.  Bitcoin's proof-of-work relies on sha256: \
             miners search for a nonce such that the block hash falls below a target \
             threshold, making block production computationally expensive while \
             verification remains trivial via a single hash computation.",
        )
        .await;

    let crypto_source = fix
        .insert_source(
            "RSA Asymmetric Cryptographic Encryption",
            "RSA is an asymmetric cryptographic public-key cipher based on the difficulty \
             of factoring the product of two large primes.  Key generation selects primes \
             p and q, computes modulus n and Euler's totient, then picks public exponent e \
             and private exponent d such that their product is congruent to 1 modulo \
             the totient.  Encryption uses modular exponentiation with the public key; \
             decryption applies modular exponentiation with the private key d.  OAEP \
             padding is required to prevent chosen-ciphertext attacks against the raw \
             RSA cipher.  The RSA asymmetric scheme underpins TLS handshakes and \
             certificate signing in public-key infrastructure.",
        )
        .await;

    // ── Topic 4: Databases ────────────────────────────────────────────────────
    // Anchor: "transactions"
    // Article-distinctive: PostgreSQL, planner, selectivity, index, MVCC
    //   NOTE: written "btree" (no hyphen) to match text-search tokenisation.
    // Source-distinctive : ACID, isolation, deadlock, normalization
    let db_article = fix
        .insert_article(
            "PostgreSQL Query Planner and Index Strategies",
            "PostgreSQL's cost-based query planner evaluates multiple execution plans \
             and selects the one with the lowest estimated cost.  The planner uses \
             statistics gathered by ANALYZE to estimate selectivity for predicates.  \
             Btree indexes support equality and range lookups; GIN indexes accelerate \
             full-text search and JSONB containment queries.  When an index covers all \
             required columns the planner avoids fetching heap tuples entirely via an \
             index-only scan.  Transactions in PostgreSQL use MVCC: each transaction \
             sees a consistent snapshot, avoiding reader-writer conflicts while the \
             PostgreSQL planner chooses the optimal access path.",
        )
        .await;

    let db_source = fix
        .insert_source(
            "ACID Transactions and Database Normalisation",
            "Database transactions guarantee Atomicity, Consistency, Isolation, and \
             Durability (ACID).  Isolation levels from READ COMMITTED to SERIALIZABLE \
             trade concurrency against anomaly prevention.  Deadlock detection breaks \
             cycles in the lock graph by aborting one of the involved transactions.  \
             Normalisation decomposes tables to eliminate redundancy: first normal form \
             prohibits repeating groups, Boyce-Codd normal form removes all non-trivial \
             functional dependencies.  PostgreSQL's MVCC implementation maintains multiple \
             row versions so readers never block writers, satisfying the isolation \
             requirement of ACID transactions without locking overhead.",
        )
        .await;

    // ── Topic 5: Genomics ─────────────────────────────────────────────────────
    // Anchor: "genomic"
    // Article-distinctive: CRISPR, Cas9, nuclease, guide, repair
    // Source-distinctive : ribosome, mRNA, codon, translation, polypeptide
    let bio_article = fix
        .insert_article(
            "CRISPR-Cas9 Genomic Editing Mechanism",
            "CRISPR-Cas9 enables precise genomic editing by directing the Cas9 nuclease \
             to a target DNA sequence via a guide RNA that matches the genomic locus \
             flanked by a protospacer adjacent motif.  Cas9 introduces a double-strand \
             break; repair via non-homologous end joining introduces indels that disrupt \
             gene function, while homology-directed repair installs a precise genomic \
             edit from a donor template.  Base editors extend CRISPR beyond breaks, \
             enabling single-nucleotide genomic substitutions without cutting both strands \
             of the DNA double helix.  Guide design determines CRISPR specificity.",
        )
        .await;

    let bio_source = fix
        .insert_source(
            "Ribosome Translation and Genomic Protein Synthesis",
            "Protein synthesis translates the genomic code from mRNA into polypeptide \
             chains at the ribosome.  The ribosome positions the mRNA codon in the A site, \
             where a cognate aminoacyl-tRNA bearing the matching anticodon delivers its \
             amino acid.  Peptidyl transferase catalyses peptide bond formation, \
             translocating the nascent polypeptide chain to the P site.  Start codons \
             specify methionine; stop codons recruit release factors.  Ribosomal quality \
             control rescues stalled ribosomes and degrades aberrant mRNA, protecting \
             cells from toxic truncated polypeptides encoded by damaged genomic templates.",
        )
        .await;

    // ── Topic 6: Quantum Computing ────────────────────────────────────────────
    // Anchor: "quantum"
    // Article-distinctive: qubit, superposition, entanglement, decoherence, Hadamard
    // Source-distinctive : Shor, Grover, factoring, amplitude, interference
    let qc_article = fix
        .insert_article(
            "Qubits, Superposition, and Quantum Entanglement",
            "A qubit is the fundamental unit of quantum information, existing as a \
             superposition of basis states until measured.  Multi-qubit systems can be \
             entangled: measuring one qubit instantaneously determines the state of its \
             entangled partner regardless of distance.  Quantum gates manipulate \
             superposition amplitudes; the Hadamard gate creates equal superposition from \
             a basis state.  Decoherence — interaction with the environment — collapses \
             quantum superposition, limiting circuit depth.  Error correction codes such \
             as the surface code protect logical qubits from physical decoherence \
             through redundant entangled ancilla measurements.",
        )
        .await;

    let qc_source = fix
        .insert_source(
            "Shor and Grover Quantum Algorithms",
            "Shor's algorithm solves integer factoring in quantum polynomial time, \
             threatening RSA cryptography by exploiting quantum Fourier transforms and \
             period finding.  Grover's algorithm provides a quadratic quantum speedup for \
             unstructured search: finding a target among N items requires square-root-N \
             quantum oracle queries via amplitude amplification.  Both algorithms leverage \
             quantum superposition and interference to concentrate amplitude on correct \
             answers.  Shor and Grover represent the two paradigmatic quantum speedups: \
             exponential for factoring and quadratic for unstructured search.",
        )
        .await;

    // ── Topic 7: Climate Science ──────────────────────────────────────────────
    // Anchor: "radiative"
    // Article-distinctive: greenhouse, CO2, albedo, paleoclimate
    // Source-distinctive : permafrost, methane, thermokarst, arctic, tipping
    let climate_article = fix
        .insert_article(
            "Greenhouse Gases and Radiative Forcing",
            "Radiative forcing quantifies the energy imbalance imposed on Earth's climate \
             system by a given perturbation.  CO2 is the dominant anthropogenic greenhouse \
             gas: its absorption bands in the infrared spectrum trap outgoing longwave \
             radiation, raising surface temperatures.  The albedo feedback amplifies \
             warming: melting polar ice exposes darker ocean and land, reducing reflectivity \
             and increasing radiative absorption.  Climate sensitivity is estimated between \
             2.5 and 4 degrees by current paleoclimate model ensembles.  Greenhouse gas \
             concentrations are tracked via the radiative forcing metric in Watts per \
             square metre relative to pre-industrial CO2 baselines.",
        )
        .await;

    let climate_source = fix
        .insert_source(
            "Arctic Permafrost and Radiative Methane Emissions",
            "Arctic permafrost stores vast quantities of organic carbon accumulated over \
             millennia.  As permafrost thaws under anthropogenic warming, microbial \
             decomposition releases CO2 and methane, a greenhouse gas with large radiative \
             warming potential over a 20-year horizon.  Thermokarst lakes form as \
             ice-rich permafrost collapses, accelerating methane ebullition from the \
             arctic substrate.  Positive radiative feedback loops between permafrost thaw \
             and surface warming risk triggering tipping points beyond which methane \
             emissions become self-sustaining.  Permafrost thaw and thermokarst formation \
             represent one of the most significant arctic climate feedbacks.",
        )
        .await;

    // ── Topic 8: Neuroscience ─────────────────────────────────────────────────
    // Anchor: "synaptic"
    // Article-distinctive: plasticity, potentiation, dendrite, axon, LTP
    // Source-distinctive : dopamine, serotonin, limbic, neurotransmitter, reward
    let neuro_article = fix
        .insert_article(
            "Synaptic Plasticity and Long-Term Potentiation",
            "Synaptic plasticity is the capacity of neural connections to strengthen or \
             weaken in response to activity, forming the cellular basis of learning and \
             memory.  Long-term potentiation (LTP) is induced when pre- and postsynaptic \
             neurons fire in close temporal proximity, causing NMDA receptor activation \
             and AMPA receptor insertion at the postsynaptic density.  Structural synaptic \
             changes include dendritic spine enlargement and new spine formation along the \
             target dendrite.  Hebbian plasticity — neurons that fire together wire \
             together — generalises LTP into a theory of associative synaptic learning.  \
             Axonal conduction velocity modulates the timing of synaptic potentiation.",
        )
        .await;

    let neuro_source = fix
        .insert_source(
            "Dopamine and Synaptic Reward Neurotransmission",
            "Dopamine is a monoamine neurotransmitter central to synaptic reward \
             prediction and motivated behaviour.  Midbrain dopaminergic neurons project \
             along the mesolimbic pathway to the nucleus accumbens, modulating synaptic \
             strength during reward learning.  Serotonin, another monoamine \
             neurotransmitter, regulates mood and sleep via serotonergic projections from \
             the raphe nuclei.  The limbic system integrates emotional salience with \
             synaptic memory consolidation via the hippocampus, linking aversive and \
             appetitive experiences to dopamine-mediated behavioural responses.  \
             Limbic dopamine release is the neurochemical substrate of reward.",
        )
        .await;

    // ── Topic 9: Economics ────────────────────────────────────────────────────
    // Anchor: "monetary"
    // Article-distinctive: quantitative, easing, inflation, Federal, Reserve
    // Source-distinctive : Keynesian, multiplier, fiscal, deficit, aggregate
    let econ_article = fix
        .insert_article(
            "Monetary Policy and Quantitative Easing",
            "Monetary policy controls the money supply and interest rates to achieve \
             macroeconomic objectives such as price stability and full employment.  \
             Central banks implement conventional monetary policy through open market \
             operations that target the federal funds rate.  When the nominal rate \
             reaches the zero lower bound, unconventional monetary tools such as \
             quantitative easing purchase long-duration assets to suppress term premia \
             and stimulate investment.  Persistent above-target inflation prompts \
             monetary tightening: rate hikes by the Federal Reserve and balance-sheet \
             runoff to reduce aggregate demand and cool price growth.",
        )
        .await;

    let econ_source = fix
        .insert_source(
            "Fiscal Policy and Keynesian Monetary Multipliers",
            "Fiscal policy uses government spending and taxation to influence aggregate \
             demand.  The Keynesian monetary multiplier captures how an initial injection \
             of government expenditure propagates through the economy: each recipient \
             saves a fraction and spends the rest, generating successive rounds of income.  \
             Deficit spending during recessions can offset private demand shortfalls, but \
             the size of the Keynesian multiplier depends on monetary accommodation and \
             Ricardian equivalence.  Supply-side fiscal reforms aim to raise potential GDP \
             rather than smooth cyclical fluctuations by adjusting fiscal deficit levels.",
        )
        .await;

    // ── Topic 10: Philosophy of Mind ─────────────────────────────────────────
    // Anchor: "phenomenal"
    // Article-distinctive: qualia, consciousness, Chalmers, epiphenomenalism
    // Source-distinctive : functionalism, physicalism, zombie, dualism
    let phil_article = fix
        .insert_article(
            "Qualia and the Phenomenal Hard Problem of Consciousness",
            "The hard problem of consciousness, articulated by Chalmers, asks why physical \
             brain processes give rise to subjective phenomenal experience.  Qualia are the \
             intrinsic phenomenal properties of conscious states: the redness of red, the \
             painfulness of pain.  Physicalist theories must either reduce qualia to \
             functional roles or accept that phenomenal properties are not captured by \
             third-person physical descriptions.  Mary's room thought experiment argues \
             that a neuroscientist who knows all physical facts still learns a phenomenal \
             fact upon seeing red.  Epiphenomenalism holds that phenomenal qualia are \
             causally inert byproducts of physical brain processes.",
        )
        .await;

    let phil_source = fix
        .insert_source(
            "Functionalism, Physicalism, and Phenomenal Dualism",
            "Functionalism holds that mental states are defined by their causal and \
             functional roles rather than their physical substrate.  Physicalism maintains \
             that all phenomenal facts supervene on physical facts, leaving no explanatory \
             gap.  Property dualism concedes that phenomenal consciousness is not reducible \
             to functional or physical properties while denying a separate mental substance.  \
             The zombie argument contends that a creature physically identical to a human \
             but lacking phenomenal experience is conceivable, challenging physicalist and \
             functionalist accounts of mind.  Epiphenomenalism and dualism both question \
             whether phenomenal zombie scenarios are metaphysically coherent.",
        )
        .await;

    // ── Query cases ───────────────────────────────────────────────────────────
    //
    // Each query is an OR-split websearch expression:
    //   "<article-side terms> OR <source-side terms>"
    //
    // websearch_to_tsquery AND-binds adjacent words within each OR clause, so
    // a document only needs to satisfy ONE side.  This lets both the article
    // and its backing source appear in results while keeping inter-topic
    // false-positive rates low.

    vec![
        BenchmarkCase {
            label: "ML: gradient descent & regularization",
            // Article side: backpropagation gradient descent convergence
            // Source side:  dropout regularization relu activation
            query: "backpropagation gradient descent convergence OR dropout regularization relu activation",
            relevant_ids: vec![ml_article, ml_source],
        },
        BenchmarkCase {
            label: "DS: Raft consensus & Kafka architecture",
            // Article side: Raft leader election quorum
            // Source side:  Kafka partition broker consumer
            query: "Raft leader election quorum OR Kafka partition broker consumer",
            relevant_ids: vec![ds_article, ds_source],
        },
        BenchmarkCase {
            label: "Crypto: hash preimage & RSA asymmetric",
            // Article side: hash preimage collision merkle  (no SHA-256 hyphen)
            // Source side:  RSA asymmetric modular exponentiation
            query: "hash preimage collision merkle OR RSA asymmetric modular exponentiation",
            relevant_ids: vec![crypto_article, crypto_source],
        },
        BenchmarkCase {
            label: "DB: PostgreSQL planner & ACID transactions",
            // Article side: PostgreSQL planner selectivity btree
            //   (content uses "btree" not "B-tree" so it tokenises as one token)
            // Source side:  ACID isolation deadlock normalisation
            query: "PostgreSQL planner selectivity btree OR ACID isolation deadlock normalization",
            relevant_ids: vec![db_article, db_source],
        },
        BenchmarkCase {
            label: "Bio: CRISPR editing & ribosome translation",
            // Article side: CRISPR nuclease guide repair
            // Source side:  ribosome codon translation polypeptide
            query: "CRISPR nuclease guide repair OR ribosome codon translation polypeptide",
            relevant_ids: vec![bio_article, bio_source],
        },
        BenchmarkCase {
            label: "QC: qubits superposition & Shor algorithm",
            // Article side: qubit superposition entanglement decoherence
            // Source side:  Shor Grover factoring amplitude
            query: "qubit superposition entanglement decoherence OR Shor Grover factoring amplitude",
            relevant_ids: vec![qc_article, qc_source],
        },
        BenchmarkCase {
            label: "Climate: greenhouse radiative & permafrost methane",
            // Article side: greenhouse CO2 albedo paleoclimate
            // Source side:  permafrost methane thermokarst arctic
            query: "greenhouse CO2 albedo paleoclimate OR permafrost methane thermokarst arctic",
            relevant_ids: vec![climate_article, climate_source],
        },
        BenchmarkCase {
            label: "Neuro: synaptic plasticity & dopamine reward",
            // Article side: potentiation dendrite LTP plasticity
            // Source side:  dopamine serotonin limbic neurotransmitter
            query: "potentiation dendrite LTP plasticity OR dopamine serotonin limbic neurotransmitter",
            relevant_ids: vec![neuro_article, neuro_source],
        },
        BenchmarkCase {
            label: "Econ: quantitative easing & Keynesian multiplier",
            // Article side: quantitative easing Federal Reserve inflation
            // Source side:  Keynesian multiplier fiscal deficit
            query: "quantitative easing Federal Reserve inflation OR Keynesian multiplier fiscal deficit",
            relevant_ids: vec![econ_article, econ_source],
        },
        BenchmarkCase {
            label: "Phil: qualia consciousness & functionalism dualism",
            // Article side: qualia phenomenal consciousness epiphenomenalism
            // Source side:  functionalism physicalism zombie dualism
            query: "qualia phenomenal consciousness epiphenomenalism OR functionalism physicalism zombie dualism",
            relevant_ids: vec![phil_article, phil_source],
        },
        BenchmarkCase {
            label: "Cross-domain: quantum factoring & cryptographic hash",
            // QC source side:     Shor quantum factoring
            // Crypto article side: cryptographic hash preimage collision
            query: "Shor quantum factoring OR cryptographic hash preimage collision",
            relevant_ids: vec![qc_source, crypto_article],
        },
    ]
}

// ─── precision helper ─────────────────────────────────────────────────────────

/// Compute Precision@K for a single query result list.
///
/// `results` is the ranked list of returned node IDs (index 0 = rank 1).
/// `relevant` is the ground-truth set of relevant node IDs.
fn precision_at_k(results: &[Uuid], relevant: &HashSet<Uuid>, k: usize) -> f64 {
    if k == 0 || results.is_empty() {
        return 0.0;
    }
    let hits = results
        .iter()
        .take(k)
        .filter(|id| relevant.contains(id))
        .count();
    hits as f64 / k as f64
}

// ─── main benchmark test ──────────────────────────────────────────────────────

/// Run the full 11-query benchmark suite and assert aggregate quality thresholds.
///
/// # Baseline thresholds
///
/// | Metric   | Value | Rationale                                              |
/// |----------|-------|--------------------------------------------------------|
/// | Mean P@1 | 0.80  | At least 9/11 queries return a relevant result at #1   |
/// | Mean P@3 | 0.55  | On average ≥ 1.65 relevant docs in top-3 per query     |
/// | Mean P@5 | 0.35  | On average ≥ 1.75 relevant docs in top-5 per query     |
///
/// Each query has exactly 2 relevant docs (article + source).  With
/// OR-split queries both should appear in results, making the ideal P@3
/// for each query 2/3 ≈ 0.67 and the ideal P@5 2/5 = 0.40.
#[tokio::test]
#[serial]
async fn search_benchmark_precision_and_latency() {
    let mut fix = TestFixture::new().await;
    let cases = build_fixture(&mut fix).await;
    let svc = SearchService::new(fix.pool.clone());

    let mut case_results: Vec<CaseResult> = Vec::with_capacity(cases.len());

    for case in &cases {
        let relevant: HashSet<Uuid> = case.relevant_ids.iter().cloned().collect();

        let req = SearchRequest {
            query: case.query.to_string(),
            // No embedding → vector dimension skipped; lexical drives ranking.
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        spreading_activation: None,
        };

        let t0 = Instant::now();
        let (search_results, _meta) = svc
            .search(req)
            .await
            .unwrap_or_else(|e| panic!("search failed for '{}': {e}", case.label));
        let latency_ms = t0.elapsed().as_millis() as u64;

        let ranked_ids: Vec<Uuid> = search_results.iter().map(|r| r.node_id).collect();

        let p1 = precision_at_k(&ranked_ids, &relevant, 1);
        let p3 = precision_at_k(&ranked_ids, &relevant, 3);
        let p5 = precision_at_k(&ranked_ids, &relevant, 5);

        eprintln!(
            "[bench] {:55}  P@1={:.2}  P@3={:.2}  P@5={:.2}  {}ms",
            case.label, p1, p3, p5, latency_ms,
        );

        case_results.push(CaseResult {
            label: case.label,
            p_at_1: p1,
            p_at_3: p3,
            p_at_5: p5,
            latency_ms,
        });
    }

    // ── Latency assertions ────────────────────────────────────────────────────

    let max_latency = case_results.iter().map(|r| r.latency_ms).max().unwrap_or(0);
    let mean_latency =
        case_results.iter().map(|r| r.latency_ms).sum::<u64>() as f64 / case_results.len() as f64;

    eprintln!("[bench] latency — mean: {mean_latency:.1}ms  max (p99 proxy): {max_latency}ms");

    for r in &case_results {
        assert!(
            r.latency_ms < 2_000,
            "Query '{}' exceeded 2 000ms latency budget: {}ms",
            r.label,
            r.latency_ms,
        );
    }

    // ── Aggregate precision assertions ────────────────────────────────────────

    let n = case_results.len() as f64;
    let mean_p1 = case_results.iter().map(|r| r.p_at_1).sum::<f64>() / n;
    let mean_p3 = case_results.iter().map(|r| r.p_at_3).sum::<f64>() / n;
    let mean_p5 = case_results.iter().map(|r| r.p_at_5).sum::<f64>() / n;

    eprintln!(
        "[bench] aggregate — Mean P@1={mean_p1:.4}  Mean P@3={mean_p3:.4}  Mean P@5={mean_p5:.4}"
    );

    // Threshold constants — update here when the search stack is intentionally improved.
    const THRESHOLD_P1: f64 = 0.80;
    const THRESHOLD_P3: f64 = 0.55;
    const THRESHOLD_P5: f64 = 0.35;

    assert!(
        mean_p1 >= THRESHOLD_P1,
        "Mean P@1 {mean_p1:.4} fell below baseline threshold {THRESHOLD_P1:.2}.\n\
         Per-query breakdown:\n{}",
        case_results
            .iter()
            .map(|r| format!("  [{:.2}] {}", r.p_at_1, r.label))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    assert!(
        mean_p3 >= THRESHOLD_P3,
        "Mean P@3 {mean_p3:.4} fell below baseline threshold {THRESHOLD_P3:.2}.\n\
         Per-query breakdown:\n{}",
        case_results
            .iter()
            .map(|r| format!("  [{:.2}] {}", r.p_at_3, r.label))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    assert!(
        mean_p5 >= THRESHOLD_P5,
        "Mean P@5 {mean_p5:.4} fell below baseline threshold {THRESHOLD_P5:.2}.\n\
         Per-query breakdown:\n{}",
        case_results
            .iter()
            .map(|r| format!("  [{:.2}] {}", r.p_at_5, r.label))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    fix.cleanup().await;
}

// ─── per-metric guard tests ───────────────────────────────────────────────────

/// Assert that each query independently retrieves at least one relevant result
/// somewhere in the top 5 (Recall@5 ≥ 1 for every case).
///
/// This catches total-miss regressions where a query returns zero relevant
/// results — failures that aggregate P@K can mask when most queries succeed.
#[tokio::test]
#[serial]
async fn search_benchmark_no_total_miss() {
    let mut fix = TestFixture::new().await;
    let cases = build_fixture(&mut fix).await;
    let svc = SearchService::new(fix.pool.clone());

    for case in &cases {
        let relevant: HashSet<Uuid> = case.relevant_ids.iter().cloned().collect();

        let req = SearchRequest {
            query: case.query.to_string(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        spreading_activation: None,
        };

        let (search_results, _meta) = svc
            .search(req)
            .await
            .unwrap_or_else(|e| panic!("search failed for '{}': {e}", case.label));

        let top5_ids: HashSet<Uuid> = search_results.iter().take(5).map(|r| r.node_id).collect();

        let hit_count = top5_ids.intersection(&relevant).count();

        assert!(
            hit_count >= 1,
            "Total miss for query '{}': none of the {} expected relevant documents \
             appeared in the top-5 results.\n\
             Query: {}\n\
             Top results: {:?}",
            case.label,
            case.relevant_ids.len(),
            case.query,
            search_results
                .iter()
                .take(5)
                .map(|r| format!("{} (score={:.4})", r.node_id, r.score))
                .collect::<Vec<_>>(),
        );
    }

    fix.cleanup().await;
}

/// Verify that individual P@3 never drops below a per-query floor of 0.30.
///
/// The floor is intentionally lenient so that one genuinely hard query does not
/// fail the suite; the aggregate test above enforces the stricter end-to-end
/// bar.  A P@3 of 0.00 almost certainly indicates a search regression rather
/// than statistical noise.
#[tokio::test]
#[serial]
async fn search_benchmark_per_query_floor() {
    let mut fix = TestFixture::new().await;
    let cases = build_fixture(&mut fix).await;
    let svc = SearchService::new(fix.pool.clone());

    for case in &cases {
        let relevant: HashSet<Uuid> = case.relevant_ids.iter().cloned().collect();

        let req = SearchRequest {
            query: case.query.to_string(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        spreading_activation: None,
        };

        let (search_results, _meta) = svc
            .search(req)
            .await
            .unwrap_or_else(|e| panic!("search failed for '{}': {e}", case.label));

        let ranked_ids: Vec<Uuid> = search_results.iter().map(|r| r.node_id).collect();
        let p3 = precision_at_k(&ranked_ids, &relevant, 3);

        assert!(
            p3 >= 0.30,
            "P@3 for query '{}' dropped below per-query floor of 0.30 (got {p3:.4}).\n\
             Query: {}",
            case.label,
            case.query,
        );
    }

    fix.cleanup().await;
}

/// Verify that each query completes within a 500ms single-query latency budget.
///
/// This is tighter than the 2 000ms aggregate wall-clock budget and catches
/// O(N²) or uncached-plan regressions early.  One warmup query is issued
/// before timing to ensure the connection pool and planner caches are primed.
#[tokio::test]
#[serial]
async fn search_benchmark_single_query_latency_budget() {
    let mut fix = TestFixture::new().await;
    let cases = build_fixture(&mut fix).await;
    let svc = SearchService::new(fix.pool.clone());

    // Warmup: prime connection pool and Postgres plan cache.
    let _ = svc
        .search(SearchRequest {
            query: "warmup query".to_string(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 1,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        spreading_activation: None,
        })
        .await;

    let mut over_budget: Vec<(&&str, u64)> = Vec::new();

    for case in &cases {
        let req = SearchRequest {
            query: case.query.to_string(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        spreading_activation: None,
        };

        let t0 = Instant::now();
        svc.search(req)
            .await
            .unwrap_or_else(|e| panic!("search failed for '{}': {e}", case.label));
        let ms = t0.elapsed().as_millis() as u64;

        if ms >= 500 {
            over_budget.push((&case.label, ms));
        }
    }

    assert!(
        over_budget.is_empty(),
        "The following queries exceeded the 500ms single-query latency budget:\n{}",
        over_budget
            .iter()
            .map(|(label, ms)| format!("  {ms}ms  {label}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    fix.cleanup().await;
}
