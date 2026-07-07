use anyhow::{Context, Result};
use regex::Regex;
use std::sync::Arc;
use tracing::info;

use crate::state::AppState;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TablePageRule {
    id: i64,
    group_id: i32,
    priority: i32,
    filter: String,
    new_value: Option<String>,
    action: String,
    description: Option<String>,
}

impl TablePageRule {
    pub(crate) async fn get_by_group(group_id: i64, app_state: Arc<AppState>) -> Result<Vec<Self>> {
        let rules = sqlx::query_as::<_, Self>(
            "SELECT * FROM rules WHERE group_id = $1 ORDER BY priority ASC",
        )
        .bind(group_id)
        .fetch_all(&app_state.db_pool)
        .await
        .context("Ошибка при получении правил для группы")?;
        Ok(rules)
    }

    pub(crate) fn process(&self, content: String) -> Result<String> {
        info!(?self.filter, "Обработка по фильтром контента");

        let rg = Regex::new(self.filter.as_str())
            .context("Невозможно скомпилировать регулярное выражение")?;
        match self.action.as_str() {
            "include" => {
                // Оставляем только совпадения, каждое с новой строки
                let matches: Vec<&str> = rg
                    .find_iter(&content)
                    .map(|m| m.as_str())
                    .filter(|s| !s.is_empty())
                    .collect();
                Ok(matches.join("\n"))
            }
            "exclude" => {
                // Заменяет все совпадения на заданное значение
                let result =
                    rg.replace_all(&content, self.new_value.as_deref().unwrap_or_default());
                Ok(result.to_string())
            }
            _ => unreachable!(),
        }
    }
}
