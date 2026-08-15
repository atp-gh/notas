//! Data models shared across the app.

use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct Notebook {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct Note {
    pub id: i64,
    pub notebook_id: Option<i64>,
    pub title: String,
    pub content: String,
    pub is_trashed: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct SearchHit {
    pub id: i64,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TagCount {
    pub id: i64,
    pub name: String,
    pub note_count: i64,
}
