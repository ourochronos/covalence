# Software Engineering Book Summaries

Key concepts from foundational software engineering books, summarized for ingestion into Covalence's knowledge graph. Full texts are commercially published and cannot be scraped.

## Software Design & Architecture

### A Philosophy of Software Design (John Ousterhout, 2018)
Core thesis: complexity is the root cause of software difficulty. Key concepts:
- **Deep modules**: modules with simple interfaces that hide significant implementation complexity. Shallow modules (complex interface, simple implementation) add complexity without value.
- **Information hiding**: the most important technique for reducing complexity. Each module should encapsulate a design decision that could change.
- **Tactical vs strategic programming**: tactical programmers optimize for getting the current task done quickly; strategic programmers invest in good design to reduce future complexity.
- **Complexity signals**: change amplification (one change requires many modifications), cognitive load (how much a developer needs to know), unknown unknowns (things developers don't know they don't know).

### Designing Data-Intensive Applications (Martin Kleppmann, 2017)
Foundational guide to distributed systems and data architecture:
- **Data models**: relational, document, graph — each with different query optimality profiles. Graph models excel at many-to-many relationships and traversal queries.
- **Storage engines**: B-trees (read-optimized) vs LSM-trees (write-optimized). Understanding storage engine internals is critical for performance tuning.
- **Replication**: single-leader, multi-leader, leaderless. Each has different consistency/availability tradeoffs.
- **Partitioning**: by key range or hash. Affects query patterns and hotspot distribution.
- **Consistency models**: linearizability, causal consistency, eventual consistency. Stronger consistency costs performance.
- **Stream processing**: event logs as the unifying abstraction. Change data capture (CDC) bridges databases and message systems.

### Domain-Driven Design (Eric Evans, 2003)
Aligning software structure with business domains:
- **Ubiquitous language**: team agrees on a shared vocabulary that appears in code, documentation, and conversation. Prevents translation errors.
- **Bounded contexts**: explicit boundaries where a domain model applies. Different contexts can have different models of the same concept.
- **Aggregates**: clusters of entities treated as a unit for data consistency. Aggregate roots control access.
- **Domain events**: something that happened in the domain that domain experts care about. Foundation for event-driven architectures.
- **Anti-corruption layer**: protects a bounded context from the models of external systems.

### Clean Architecture (Robert C. Martin, 2017)
Dependency inversion at the architectural level:
- **Dependency Rule**: source code dependencies must point inward, toward higher-level policies. Business rules don't depend on UI or database.
- **Entities**: encapsulate enterprise-wide business rules. Most stable layer.
- **Use Cases**: application-specific business rules. Orchestrate the flow of data to and from entities.
- **Interface Adapters**: convert data between use case format and external format (DB, web, etc.).
- **Frameworks and Drivers**: outermost layer. Details that can be swapped without affecting business logic.

### Software Architecture: The Hard Parts (Ford, Richards, Sadalage, Dehghani, 2021)
Practical tradeoffs in distributed architecture:
- **Saga pattern**: managing distributed transactions through choreography (events) or orchestration (coordinator).
- **Data decomposition**: when to split databases along service boundaries and how to handle cross-service queries.
- **Coupling spectrum**: from data coupling (weakest) to content coupling (strongest). Goal is appropriate coupling, not zero coupling.
- **Architecture fitness functions**: automated checks that architecture characteristics (performance, security, modularity) are maintained as the system evolves.

## Design Patterns

### Design Patterns (Gamma, Helm, Johnson, Vlissides — "Gang of Four", 1994)
Foundational catalog of reusable design solutions:
- **Strategy pattern**: define a family of algorithms, encapsulate each one, make them interchangeable. Covalence uses this for Embedder, Extractor, Resolver traits.
- **Observer pattern**: one-to-many dependency between objects. When one changes, dependents are notified. Foundation for event-driven systems.
- **Template Method**: define the skeleton of an algorithm, defer steps to subclasses. Used in pipeline stages.
- **Builder pattern**: separate construction of complex objects from their representation. Used throughout Covalence's service construction (with_embedder, with_resolver, etc.).
- **Facade pattern**: provide a simplified interface to a complex subsystem. SearchService is a facade over 6 search dimensions + fusion + reranking.
- **Decorator pattern**: attach additional responsibilities dynamically. Used for middleware in Axum.
- **Repository pattern**: mediate between domain and data mapping layers. Covalence's trait-based repos (SourceRepo, NodeRepo, etc.).

### Patterns of Enterprise Application Architecture (Fowler, 2002)
Enterprise-scale patterns:
- **Unit of Work**: maintains a list of objects affected by a transaction and coordinates writing out changes. Used in PG transactions.
- **Identity Map**: ensures each object is loaded only once per transaction. Prevents duplicate nodes in entity resolution.
- **Data Mapper**: moves data between objects and database, keeping them independent. Covalence's `source_from_row`, `node_from_row` functions.
- **Service Layer**: defines an application's boundary with a layer of services that establishes a set of available operations. Covalence's SourceService, SearchService, AdminService.

## Best Practices

### Accelerate (Forsgren, Humble, Kim, 2018)
Data-driven DevOps performance science:
- **DORA metrics**: deployment frequency, lead time for changes, change failure rate, time to restore service. The four key metrics that predict organizational performance.
- **Continuous delivery capabilities**: version control, trunk-based development, continuous integration, automated testing, deployment automation.
- **Culture matters**: generative culture (high cooperation, shared risks, bridging encouraged) outperforms pathological (power-oriented, low cooperation) and bureaucratic (rule-oriented).
- **Technical practices drive performance**: loosely coupled architecture, empowered teams, continuous delivery are the strongest predictors of both delivery speed and stability.

### Site Reliability Engineering (Beyer, Jones, Petoff, Murphy — Google, 2016)
Treating operations as a software problem:
- **Error budgets**: the acceptable amount of unreliability. If a service has a 99.9% SLO, the error budget is 0.1% — about 8.7 hours/year. When consumed, focus shifts to reliability over features.
- **Toil**: repetitive, manual, automatable operational work that scales linearly with service growth. SRE's goal is to eliminate toil through automation.
- **Blameless postmortems**: after every significant incident, produce a written record that focuses on what happened and how to prevent recurrence — not who caused it.
- **Release engineering**: hermetic builds, reproducible releases, trunk-based development with feature flags.

## Security

### Threat Modeling (Adam Shostack, 2014)
Systematic security analysis:
- **STRIDE model**: Spoofing, Tampering, Repudiation, Information disclosure, Denial of service, Elevation of privilege.
- **Data flow diagrams**: map how data moves through the system to identify trust boundaries and attack surfaces.
- **Attack trees**: hierarchical decomposition of how an attacker could achieve a goal.
- **"What can go wrong?"**: the central question of threat modeling. Applied at design time, not post-deployment.

## Rust-Specific

### Rust Atomics and Locks (Mara Bos, 2023)
Deep dive into concurrent programming in Rust:
- **Memory ordering**: Relaxed, Acquire, Release, AcqRel, SeqCst — each provides different guarantees about operation visibility across threads.
- **Lock-free data structures**: compare-and-swap (CAS) operations enable progress guarantees without mutex locks.
- **RwLock design**: multiple readers XOR one writer. Covalence uses `Arc<RwLock<GraphSidecar>>` for the shared graph.
- **Channels**: MPSC (tokio::sync::mpsc) for async message passing. Semaphores for concurrency limiting (Covalence's retry queue).
