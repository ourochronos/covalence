//! Request and response DTOs for the API.
//!
//! Organized into per-handler submodules. All types are re-exported
//! from this module with explicit named imports so that each type's
//! origin is traceable.

mod admin;
mod analysis;
mod ask;
mod common;
mod edges;
mod extensions;
mod graph;
mod hooks;
mod nodes;
mod search;
mod sessions;
mod sources;

pub use admin::{
    BackfillResponse, BridgeRequest, BridgeResponse, ClearDeadRequest, ClearDeadResponse,
    CodeSummaryResponse, ConfigAuditResponse, ConsolidateResponse, CooccurrenceRequest,
    CooccurrenceResponse, DeadJobResponse, GcResponse, HealthResponse, InvalidatedEdgeNodeResponse,
    InvalidatedEdgeStatsParams, InvalidatedEdgeStatsResponse, InvalidatedEdgeTypeResponse,
    KnowledgeGapItem, KnowledgeGapParams, KnowledgeGapsResponse, ListDeadParams, ListDeadResponse,
    MetricsResponse, NoiseCleanupRequest, NoiseCleanupResponse, NoiseEntityItem,
    OntologyClusterItem, OntologyClusterRequest, OntologyClusterResponse, PublishResponse,
    QueueStatusResponse, QueueStatusRowResponse, RaptorResponse, ReloadResponse,
    ResurrectDeadResponse, RetryFailedRequest, RetryFailedResponse, SeedOpinionsResponse,
    ServiceHealthResponse, Tier5ResolveRequest, Tier5ResolveResponse,
};
pub use analysis::{
    AffectedNodeResponse, AlignmentReportResponse, AlignmentRequest, BlastRadiusHopResponse,
    BlastRadiusRequest, BlastRadiusResponse, BootstrapResponse, CounterArgumentResponse,
    CoverageItemResponse, CoverageResponse, CritiqueEvidenceResponse, CritiqueRequest,
    CritiqueResponse, CritiqueSynthesisResponse, DivergentNodeResponse, ErosionItemResponse,
    ErosionRequest, ErosionResponse, LinkDomainsRequest, LinkDomainsResponse,
    SupportingArgumentResponse, VerificationMatchResponse, VerifyRequest, VerifyResponse,
    WhitespaceGapResponse, WhitespaceNodeResponse, WhitespaceRequest, WhitespaceResponse,
};
pub use ask::{AskApiRequest, AskApiResponse, CitationResponse};
pub use common::{AuditLogResponse, CurationResponse, FeedbackResponse, PaginationParams};
pub use edges::{CorrectEdgeRequest, DeleteEdgeParams, EdgeResponse};
pub use extensions::{ListExtensionsResponse, ReloadExtensionsResponse};
pub use graph::{
    CommunityParams, CommunityResponse, DomainLinkResponse, DomainResponse, GraphStatsResponse,
    TopologyResponse,
};
pub use hooks::{
    CreateHookRequest, CreateHookResponse, DeleteHookResponse, HookResponse, ListHooksResponse,
};
pub use nodes::{
    AnnotateNodeRequest, CorrectNodeRequest, GetNodeParams, MergeNodesRequest, MergeNodesResponse,
    NeighborhoodParams, NodeDetailResponse, NodeExplanation, NodeResponse, ProvenanceResponse,
    ResolveNodeRequest, SplitNodeRequest, SplitNodeResponse, SplitSpecRequest,
};
pub use search::{
    ContextItemResponse, ContextResponse, RelatedEntityResponse, SearchApiResponse,
    SearchFeedbackRequest, SearchGranularity, SearchMode, SearchRequest, SearchResultResponse,
    SearchTraceResponse, TraceReplayResponse,
};
pub use sessions::{
    AddTurnRequest, CreateSessionRequest, GetTurnsParams, SessionResponse, TurnResponse,
};
pub use sources::{
    ChunkResponse, CreateSourceRequest, CreateSourceResponse, DeleteSourceResponse,
    EnqueueReprocessResponse, ReprocessSourceResponse, SourceResponse,
};
