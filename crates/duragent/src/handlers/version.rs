use axum::Json;

use crate::build_info::BuildInfo;

pub async fn version() -> Json<BuildInfo> {
    Json(BuildInfo::default())
}
