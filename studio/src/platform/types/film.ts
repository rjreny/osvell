export type LibraryCoverage = {
  uniqueMovies: number;
  watchlistMovies: number;
  totalViewings: number;
  ratingEvents: number;
  unresolvedMovies: number;
  source: "export" | "rss" | "mixed" | "none";
  fullHistoryAvailable: boolean;
  rssWindowLimit?: number;
  lastFullImport?: string;
  warnings: string[];
};

export type AppSession = {
  selfUsername: string | null;
  friendCount: number;
  hasSetup: boolean;
  coverage: LibraryCoverage;
  lastRssSyncAt?: string | null;
  rssPausedUntil?: string | null;
};

export type InstallInfo = {
  version: string;
  installKind: "dev" | "installed" | "portable" | "unknown";
  appDataDir: string;
  databasePath: string;
  executablePath: string | null;
  uninstallerPath: string | null;
  logPath: string;
  dataBytes: number;
};

export type TmdbKeyStatus = {
  stored: boolean;
  valid: boolean | null;
  kind: string | null;
  lastError: string | null;
};

export type EnrichReport = {
  hasKey: boolean;
  keyValid: boolean | null;
  attempted: number;
  matched: number;
  posters: number;
  remainingUnmatched: number;
  remainingWithoutPoster: number;
  errors: number;
  lastError: string | null;
  logPath: string | null;
};

export type JobProgress = {
  job: string;
  label: string;
  current: number;
  total: number;
  posters: number;
  errors: number;
  done: boolean;
  detail?: string | null;
  enrich?: EnrichReport | null;
  import?: ImportResult | null;
  taste?: TasteReport | null;
  feeds?: FeedSyncReport | null;
};

export type LibraryItem = {
  id: string;
  title: string;
  year: number | null;
  currentRating: number | null;
  poster: string | null;
  backdrop?: string | null;
  overview?: string | null;
  watched: boolean;
  watchlist: boolean;
  liked: boolean;
  viewingCount: number;
  matchState: string;
  sourceType: string;
  lastWatchedAt: string | null;
};

export type LibraryPage = {
  items: LibraryItem[];
  total: number;
  coverage: LibraryCoverage;
};

export type StatsBucket = {
  label: string;
  count: number;
  averageRating: number | null;
};

export type StatsSnapshot = {
  viewingMonths: StatsBucket[];
  genres: StatsBucket[];
  rewatchCount: number;
  totalRuntimeMinutes: number;
  runtimeViewings: number;
  metadataMovies: number;
};

export type ViewingHistoryItem = {
  id: string;
  occurredAt: string | null;
  publishedAt: string | null;
  rewatch: boolean;
  rating: number | null;
  source: string;
};

export type FriendActivityItem = {
  username: string;
  title: string;
  year: number | null;
  rating: number | null;
  review: string | null;
  watchedAt: string | null;
  publishedAt: string | null;
  poster: string | null;
};

export type FilmDetail = {
  id: string;
  title: string;
  year: number | null;
  currentRating: number | null;
  poster: string | null;
  backdrop: string | null;
  overview: string | null;
  runtime: number | null;
  genres: string[];
  matchState: string;
  sourceIdentity: string;
  yourHistory: ViewingHistoryItem[];
  friends: FriendActivityItem[];
  tmdbId?: number | null;
  tmdbVoteAverage: number | null;
  tmdbVoteCount: number | null;
  tmdbReviews: string[];
  tagline?: string | null;
  directors?: string[];
  cast: FilmCastMember[];
  crew: FilmCrewMember[];
  companies: ProductionCompany[];
  keywords: string[];
  connections: FilmConnection[];
  collectionName?: string | null;
  collection?: LibraryItem[];
  similar: LibraryItem[];
  trailers?: FilmTrailer[];
};

export type FilmTrailer = {
  key: string;
  name: string;
  site: string;
  type: string;
  official?: boolean;
};

export type ArtworkImage = {
  path: string;
  url: string;
};

export type FilmArtwork = {
  posters: ArtworkImage[];
  backdrops: ArtworkImage[];
  selectedPoster: string | null;
  selectedBackdrop: string | null;
  defaultPoster: string | null;
  defaultBackdrop: string | null;
};

export type FilmCastMember = {
  tmdbId: number | null;
  name: string;
  profile: string | null;
  character: string | null;
  order: number | null;
};

export type FilmCrewMember = {
  tmdbId: number | null;
  name: string;
  profile: string | null;
  department: string | null;
  job: string;
};

export type ProductionCompany = {
  tmdbId: number | null;
  name: string;
  logo: string | null;
  originCountry: string | null;
};

export type ConnectionFilm = {
  id: string;
  title: string;
  rating: number;
  poster: string | null;
};

export type FilmConnection = {
  entityKind: "person" | "company";
  entityId: string;
  name: string;
  roles: string[];
  sharedCount: number;
  averageRating: number;
  confidence: string;
  tone: "positive" | "mixed" | "negative" | "unknown";
  evidence: ConnectionFilm[];
};

export type FilmTasteFit = {
  available: boolean;
  score: number | null;
  band: "strong" | "mixed" | "weak" | "notEnoughEvidence";
  evidenceGrade: "none" | "medium" | "strong";
  semanticFit: number;
  semanticCoverage: boolean;
  supportingSignals: string[];
  counterSignals: string[];
  evidenceTitles: string[];
  watched: boolean;
  unavailableReason: string | null;
};

export type HomeViewModel = {
  coverage: LibraryCoverage;
  recent: LibraryItem[];
  topRated: LibraryItem[];
  friendFeed: FriendActivityItem[];
};

export type ImportResult = {
  importId: string;
  movies: number;
  viewings: number;
  ratings: number;
  skipped: number;
  warnings: string[];
  coverage: LibraryCoverage;
};

export type FeedSyncReport = {
  selfSynced: boolean;
  friendsSynced: number;
  entriesAdded: number;
  skipped: boolean;
  lastSyncAt?: string | null;
  pausedUntil?: string | null;
  errors: string[];
};

export type LibraryQuery = {
  search?: string;
  sort?: "recent" | "rating" | "title" | "year";
  filter?: "all" | "watched" | "watchlist" | "unresolved";
  limit?: number;
  offset?: number;
};

export type FriendRow = {
  id: string;
  username: string;
  lastSyncAt: string | null;
  lastSyncError: string | null;
};

export type TasteKeyStatus = {
  stored: boolean;
  valid: boolean | null;
  lastError: string | null;
  model: string;
  web: boolean;
  models: TasteModelInfo[];
};

export type TasteModelInfo = {
  id: string;
  label: string;
  blurb: string;
  context: string;
  cost: string;
};

export type TasteAffinity = {
  label: string;
  evidence: string;
};

export type TasteDimension = {
  name: string;
  take: string;
};

export type TasteMatchedFeature = {
  featureKey?: string;
  name: string;
  family: string;
  appearances: number;
  recommendationMean: number;
  scoringAffinity: number;
  positiveWeight?: number;
  negativeWeight?: number;
  feedbackAdjustment?: number;
  polarizing?: boolean;
  confidence: number;
  portability: number;
  citeable: boolean;
  cited: boolean;
};

export type TasteMoodSignature = {
  modes: string[];
  thematicKeywords: string[];
};

export type TasteFeatureExposureCount = {
  featureKey: string;
  exposures: number;
};

export type TasteAttribution = {
  exposureId: string;
  runId: string;
  tmdbId: number;
  title: string;
  evidenceGrade: "none" | "medium" | "strong" | string;
  citedPositive: TasteMatchedFeature[];
  citedNegative: TasteMatchedFeature[];
  seedFilms: string[];
  semanticFit: number;
  diversityAdjustment: number;
  retrievalSource: string;
  rankingRationale: string[];
  moodSignature: TasteMoodSignature;
  priorCandidateExposures: number;
  priorFeatureExposures: TasteFeatureExposureCount[];
};

export type TasteEligibility = {
  portableEvidenceRequired: boolean;
  passed: boolean;
  passedBecause: string[];
  candidateFit?: number;
  evidenceGrade?: "none" | "medium" | "strong";
};

export type TasteEvidence = {
  title: string;
  filmId: string | null;
  tmdbId: number | null;
  poster: string | null;
};

export type TastePick = {
  title: string;
  year: number | null;
  poster: string | null;
  why: string;
  rhymesWith: string[];
  filmId: string | null;
  tmdbId: number | null;
  source: string;
  reasons?: string[];
  scoringReasons?: string[];
  evidence?: string[];
  evidenceItems?: TasteEvidence[];
  mode?: string | null;
  origin?: string | null;
  originLabel?: string | null;
  originDisplay?: string | null;
  matchedFeatures?: TasteMatchedFeature[];
  hiddenFeatures?: TasteMatchedFeature[];
  eligibility?: TasteEligibility;
  matchScore?: number;
  thinEvidence?: boolean;
  semanticFit?: number;
  semanticCoverage?: boolean;
  attribution?: TasteAttribution | null;
  /** Library state only — does not affect rank. */
  watchlist?: boolean;
};

export type TasteFeedback = {
  contentKey: string;
  tmdbId: number;
  mediaKind: string;
  action: "interested" | "rejected" | "seen" | string;
  reason?: string | null;
  suppressedUntil?: string | null;
  createdAt: string;
  updatedAt: string;
};

export type TasteException = {
  title: string;
  tmdbId?: number | null;
  rating: number;
  observedPreference: number;
  expectedPreference: number;
  residual: number;
  matchingFeatures: string[];
  evidenceDomains: string[];
  supportingFilms: string[];
  opposingFilms: string[];
};

export type TasteEvidenceCombination = {
  firstFeature: string;
  secondFeature: string;
  firstFamily: string;
  secondFamily: string;
  supportingFilms: string[];
};

export type TasteDiagnostics = {
  exceptions: TasteException[];
  evidenceCombinations: TasteEvidenceCombination[];
};

export type TasteFeatureExposureMetric = {
  featureKey: string;
  exposures: number;
  feedbackEvents: number;
};

export type TasteObservationSummary = {
  feedbackEvents: number;
  laterOutcomes: number;
  feedbackReasons: number;
  exposureCount: number;
  moodSignatureEligible: number;
  moodFallbacks: number;
  phaseTwoUnlocked: boolean;
  featureExposure: TasteFeatureExposureMetric[];
};

export type TasteStat = {
  label: string;
  count: number;
  avg: number;
  affinity?: number | null;
};

export type TasteSnapshot = {
  ratedCount: number;
  lovedCount: number;
  hatedCount: number;
  avgRating: number | null;
  genres: TasteStat[];
  decades: TasteStat[];
  directors: TasteStat[];
  cinematographers?: TasteStat[];
};

export type TasteReport = {
  title: string;
  summary: string;
  affinities: TasteAffinity[];
  aversions: TasteAffinity[];
  dimensions: TasteDimension[];
  newPicks?: TastePick[];
  explorePicks?: TastePick[];
  watchlistPicks?: TastePick[];
  picks: TastePick[];
  model: string;
  generatedAt: string;
  ratedCount: number;
  webUsed?: boolean;
  note?: string | null;
  runLogPath?: string | null;
  runId?: string;
  diagnostics?: TasteDiagnostics;
};

export type TasteState = {
  key: TasteKeyStatus;
  snapshot: TasteSnapshot;
  report: TasteReport | null;
  reportStale?: boolean;
  feedback?: TasteFeedback[];
  observation?: TasteObservationSummary;
};
