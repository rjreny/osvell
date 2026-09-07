use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCoverage {
    pub unique_movies: u32,
    pub watchlist_movies: u32,
    pub total_viewings: u32,
    pub rating_events: u32,
    pub unresolved_movies: u32,
    pub source: String,
    pub full_history_available: bool,
    pub rss_window_limit: Option<u32>,
    pub last_full_import: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSession {
    pub self_username: Option<String>,
    pub friend_count: u32,
    pub has_setup: bool,
    pub coverage: LibraryCoverage,
    #[serde(default)]
    pub last_rss_sync_at: Option<String>,
    #[serde(default)]
    pub rss_paused_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub version: String,
    pub install_kind: String,
    pub app_data_dir: String,
    pub database_path: String,
    pub executable_path: Option<String>,
    pub uninstaller_path: Option<String>,
    pub log_path: String,
    pub data_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmdbKeyStatus {
    pub stored: bool,
    pub valid: Option<bool>,
    pub kind: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichReport {
    pub has_key: bool,
    pub key_valid: Option<bool>,
    pub attempted: u32,
    pub matched: u32,
    pub posters: u32,
    pub remaining_unmatched: u32,
    pub remaining_without_poster: u32,
    pub errors: u32,
    pub last_error: Option<String>,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job: String,
    pub label: String,
    pub current: u32,
    pub total: u32,
    pub posters: u32,
    pub errors: u32,
    pub done: bool,
    /// Optional secondary status line (candidate counts, model name, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrich: Option<EnrichReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taste: Option<crate::taste::TasteReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feeds: Option<FeedSyncReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub import_id: String,
    pub movies: u32,
    pub viewings: u32,
    pub ratings: u32,
    pub skipped: u32,
    pub warnings: Vec<String>,
    pub coverage: LibraryCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDiagnostics {
    pub imports: Vec<ImportSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub id: String,
    pub content_hash: String,
    pub imported_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub username: String,
    pub entries_seen: u32,
    pub entries_added: u32,
    pub coverage: LibraryCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendSyncResult {
    pub friends_synced: u32,
    pub entries_added: u32,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedSyncReport {
    pub self_synced: bool,
    pub friends_synced: u32,
    pub entries_added: u32,
    pub skipped: bool,
    pub last_sync_at: Option<String>,
    pub paused_until: Option<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryQuery {
    pub search: Option<String>,
    pub sort: Option<String>,
    pub filter: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub current_rating: Option<f64>,
    pub poster: Option<String>,
    #[serde(default)]
    pub backdrop: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub watched: bool,
    #[serde(default)]
    pub watchlist: bool,
    #[serde(default)]
    pub liked: bool,
    #[serde(default)]
    pub viewing_count: u32,
    #[serde(default)]
    pub match_state: String,
    #[serde(default)]
    pub source_type: String,
    pub last_watched_at: Option<String>,
}

impl LibraryItem {
    pub fn catalog(
        id: String,
        title: String,
        year: Option<i32>,
        poster: Option<String>,
        backdrop: Option<String>,
        overview: Option<String>,
    ) -> Self {
        Self {
            id,
            title,
            year,
            current_rating: None,
            poster,
            backdrop,
            overview,
            watched: false,
            watchlist: false,
            liked: false,
            viewing_count: 0,
            match_state: "catalog".into(),
            source_type: "tmdb".into(),
            last_watched_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    pub total: u32,
    pub coverage: LibraryCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsBucket {
    pub label: String,
    pub count: u32,
    pub average_rating: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSnapshot {
    pub viewing_months: Vec<StatsBucket>,
    pub genres: Vec<StatsBucket>,
    pub rewatch_count: u32,
    pub total_runtime_minutes: u32,
    pub runtime_viewings: u32,
    pub metadata_movies: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewingHistoryItem {
    pub id: String,
    pub occurred_at: Option<String>,
    pub published_at: Option<String>,
    pub rewatch: bool,
    pub rating: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendActivityItem {
    pub username: String,
    pub title: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub review: Option<String>,
    pub watched_at: Option<String>,
    pub published_at: Option<String>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmDetail {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub current_rating: Option<f64>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub overview: Option<String>,
    pub runtime: Option<i32>,
    pub genres: Vec<String>,
    pub match_state: String,
    pub source_identity: String,
    pub your_history: Vec<ViewingHistoryItem>,
    pub friends: Vec<FriendActivityItem>,
    pub tmdb_id: Option<i64>,
    pub tmdb_vote_average: Option<f64>,
    pub tmdb_vote_count: Option<i32>,
    pub tmdb_reviews: Vec<String>,
    pub tagline: Option<String>,
    pub directors: Vec<String>,
    pub cast: Vec<FilmCastMember>,
    pub crew: Vec<FilmCrewMember>,
    pub companies: Vec<ProductionCompany>,
    pub keywords: Vec<String>,
    pub connections: Vec<FilmConnection>,
    pub collection_name: Option<String>,
    pub collection: Vec<LibraryItem>,
    pub similar: Vec<LibraryItem>,
    #[serde(default)]
    pub trailers: Vec<FilmTrailer>,
    #[serde(skip)]
    pub collection_hydrated: bool,
    #[serde(skip)]
    pub detail_metadata_hydrated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmTrailer {
    pub key: String,
    pub name: String,
    pub site: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub official: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmCastMember {
    #[serde(default, alias = "id")]
    pub tmdb_id: Option<i64>,
    pub name: String,
    #[serde(default, alias = "profile_path")]
    pub profile: Option<String>,
    #[serde(default)]
    pub character: Option<String>,
    #[serde(default)]
    pub order: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmCrewMember {
    #[serde(default, alias = "id")]
    pub tmdb_id: Option<i64>,
    pub name: String,
    #[serde(default, alias = "profile_path")]
    pub profile: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
    pub job: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductionCompany {
    #[serde(default, alias = "id")]
    pub tmdb_id: Option<i64>,
    pub name: String,
    #[serde(default, alias = "logo_path")]
    pub logo: Option<String>,
    #[serde(default, alias = "origin_country")]
    pub origin_country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFilm {
    pub id: String,
    pub title: String,
    pub rating: f64,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmConnection {
    pub entity_kind: String,
    pub entity_id: String,
    pub name: String,
    pub roles: Vec<String>,
    pub shared_count: u32,
    pub average_rating: f64,
    pub confidence: String,
    pub tone: String,
    pub evidence: Vec<ConnectionFilm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeViewModel {
    pub coverage: LibraryCoverage,
    pub recent: Vec<LibraryItem>,
    pub top_rated: Vec<LibraryItem>,
    pub friend_feed: Vec<FriendActivityItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRatingInput {
    pub id: String,
    pub rating: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkImage {
    pub path: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmArtwork {
    pub posters: Vec<ArtworkImage>,
    pub backdrops: Vec<ArtworkImage>,
    pub selected_poster: Option<String>,
    pub selected_backdrop: Option<String>,
    pub default_poster: Option<String>,
    pub default_backdrop: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetArtworkInput {
    pub id: String,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationResult {
    pub status: String,
    pub migration_version: i32,
    pub validation_result: String,
    pub coverage: LibraryCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyLibrary {
    pub username: Option<String>,
    pub films: Option<Vec<LegacyFilm>>,
    pub friends: Option<Vec<LegacyFriend>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFilm {
    pub key: Option<String>,
    pub name: String,
    pub year: Option<i32>,
    pub uri: Option<String>,
    pub rating: Option<f64>,
    pub watched: Option<bool>,
    pub watchlist: Option<bool>,
    pub liked: Option<bool>,
    pub rewatch: Option<bool>,
    pub watched_date: Option<String>,
    pub tmdb_id: Option<String>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyFriend {
    pub username: String,
    pub entries: Vec<LegacyFriendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFriendEntry {
    pub name: String,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub liked: Option<bool>,
    pub watched_date: Option<String>,
    pub link: Option<String>,
    pub poster: Option<String>,
}
