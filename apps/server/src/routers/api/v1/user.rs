use axum::{
	extract::{Path, State},
	middleware::from_extractor_with_state,
	routing::{delete, get, put},
	Json, Router,
};
use axum_extra::extract::Query;
use prisma_client_rust::{chrono::Utc, Direction};
use serde::Deserialize;
use specta::Type;
use stump_core::{
	db::{
		entity::{AgeRestriction, LoginActivity, User, UserPermission, UserPreferences},
		query::pagination::{Pageable, Pagination, PaginationQuery},
	},
	prisma::{
		age_restriction, session, user, user_login_activity, user_preferences,
		PrismaClient,
	},
};
use tower_sessions::Session;
use tracing::{debug, trace};
use utoipa::ToSchema;

use crate::{
	config::{session::SESSION_USER_KEY, state::AppState},
	errors::{ApiError, ApiResult},
	middleware::auth::Auth,
	utils::{
		get_hash_cost, get_session_server_owner_user, get_session_user, UserQueryRelation,
	},
};

pub(crate) fn mount(app_state: AppState) -> Router<AppState> {
	Router::new()
		.route("/users", get(get_users).post(create_user))
		.route(
			"/users/login-activity",
			get(get_user_login_activity).delete(delete_user_login_activity),
		)
		.nest(
			"/users/me",
			Router::new()
				.route("/", put(update_current_user))
				.route("/preferences", put(update_current_user_preferences)),
		)
		.nest(
			"/users/:id",
			Router::new()
				.route(
					"/",
					get(get_user_by_id)
						.put(update_user_handler)
						.delete(delete_user_by_id),
				)
				.route("/sessions", delete(delete_user_sessions))
				.route("/lock", put(update_user_lock_status))
				.route("/login-activity", get(get_user_login_activity_by_id))
				.route(
					"/preferences",
					get(get_user_preferences).put(update_user_preferences),
				),
		)
		.layer(from_extractor_with_state::<Auth, AppState>(app_state))
}

pub(crate) fn apply_pagination<'a>(
	query: user::FindMany<'a>,
	pagination: &Pagination,
) -> user::FindMany<'a> {
	match pagination {
		Pagination::Page(page_query) => {
			let (skip, take) = page_query.get_skip_take();
			query.skip(skip).take(take)
		},
		Pagination::Cursor(cursor_params) => {
			let mut cursor_query = query;
			if let Some(cursor) = cursor_params.cursor.as_deref() {
				cursor_query = cursor_query
					.cursor(user::id::equals(cursor.to_string()))
					.skip(1);
			}
			if let Some(limit) = cursor_params.limit {
				cursor_query = cursor_query.take(limit);
			}
			cursor_query
		},
		_ => query,
	}
}

#[utoipa::path(
	get,
	path = "/api/v1/users",
	tag = "user",
	params(
		("pagination_query" = Option<PaginationQuery>, Query, description = "The pagination query"),
		("relation_query" = Option<UserQueryRelation>, Query, description = "The relations to include"),
	),
	responses(
		(status = 200, description = "Successfully fetched users", body = [User]),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn get_users(
	State(ctx): State<AppState>,
	relation_query: Query<UserQueryRelation>,
	pagination_query: Query<PaginationQuery>,
	session: Session,
) -> ApiResult<Json<Pageable<Vec<User>>>> {
	get_session_server_owner_user(&session)?;

	let pagination = pagination_query.0.get();
	let is_unpaged = pagination.is_unpaged();
	let pagination_cloned = pagination.clone();

	tracing::debug!(?relation_query, "get_users");

	let include_user_read_progress =
		relation_query.include_read_progresses.unwrap_or_default();
	let include_session_count = relation_query.include_session_count.unwrap_or_default();
	let include_restrictions = relation_query.include_restrictions.unwrap_or_default();

	let (users, count) = ctx
		.db
		._transaction()
		.run(|client| async move {
			let mut query = client.user().find_many(vec![]);

			if include_user_read_progress {
				query = query.with(user::read_progresses::fetch(vec![]));
			}

			if include_session_count {
				query = query.with(user::sessions::fetch(vec![]));
			}

			if include_restrictions {
				query = query.with(user::age_restriction::fetch());
			}

			if !is_unpaged {
				query = apply_pagination(query, &pagination_cloned);
			}

			let users = query.exec().await?.into_iter().map(User::from).collect();

			if is_unpaged {
				return Ok((users, None));
			}

			client
				.user()
				.count(vec![])
				.exec()
				.await
				.map(|count| (users, Some(count)))
		})
		.await?;

	if let Some(count) = count {
		return Ok(Json(Pageable::from((users, count, pagination))));
	}

	Ok(Json(Pageable::from(users)))
}

// TODO: pagination!
#[utoipa::path(
	get,
	path = "/api/v1/users/login-activity",
	tag = "user",
	responses(
		(status = 200, description = "Successfully fetched login activity"),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn get_user_login_activity(
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<Json<Vec<LoginActivity>>> {
	get_session_server_owner_user(&session)?;

	let client = ctx.get_db();

	let user_activity = client
		.user_login_activity()
		.find_many(vec![])
		.with(user_login_activity::user::fetch())
		.order_by(user_login_activity::timestamp::order(Direction::Desc))
		.exec()
		.await?;

	Ok(Json(
		user_activity.into_iter().map(LoginActivity::from).collect(),
	))
}

#[utoipa::path(
	delete,
	path = "/api/v1/users/login-activity",
	tag = "user",
	responses(
		(status = 200, description = "Successfully deleted user login activity"),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn delete_user_login_activity(
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<Json<()>> {
	get_session_server_owner_user(&session)?;

	let client = ctx.get_db();

	client
		.user_login_activity()
		.delete_many(vec![])
		.exec()
		.await?;

	Ok(Json(()))
}

#[derive(Deserialize, Type, ToSchema)]
pub struct UpdateUser {
	pub username: String,
	pub password: Option<String>,
	pub avatar_url: Option<String>,
	#[serde(default)]
	pub permissions: Vec<UserPermission>,
	pub age_restriction: Option<AgeRestriction>,
}

async fn update_user(
	by_user: &User,
	client: &PrismaClient,
	for_user_id: String,
	input: UpdateUser,
) -> ApiResult<User> {
	let mut update_params = vec![
		user::username::set(input.username),
		user::avatar_url::set(input.avatar_url),
	];
	if let Some(password) = input.password {
		let hashed_password = bcrypt::hash(password, get_hash_cost())?;
		update_params.push(user::hashed_password::set(hashed_password));
	}

	let to_update_is_server_owner = by_user.is_server_owner && by_user.id == for_user_id;
	if to_update_is_server_owner {
		let updated_user_data = client
			.user()
			.update(user::id::equals(for_user_id), update_params)
			// NOTE: we have to fetch preferences because if we update session without
			// it then it effectively removes the preferences from the session
			.with(user::user_preferences::fetch())
			.exec()
			.await?;

		Ok(User::from(updated_user_data))
	} else {
		let updated_user_data = client
			._transaction()
			.run(|tx| async move {
				if let Some(age_restriction) = input.age_restriction {
					let upserted_age_restriction = tx
						.age_restriction()
						.upsert(
							age_restriction::user_id::equals(for_user_id.clone()),
							(
								age_restriction.age,
								user::id::equals(for_user_id.clone()),
								vec![age_restriction::restrict_on_unset::set(
									age_restriction.restrict_on_unset,
								)],
							),
							vec![
								age_restriction::age::set(age_restriction.age),
								age_restriction::restrict_on_unset::set(
									age_restriction.restrict_on_unset,
								),
							],
						)
						.exec()
						.await?;
					tracing::trace!(?upserted_age_restriction, "Upserted age restriction")
				}

				update_params.push(user::permissions::set(Some(
					input
						.permissions
						.into_iter()
						.map(|p| p.to_string())
						.collect::<Vec<String>>()
						.join(","),
				)));

				tx.user()
					.update(user::id::equals(for_user_id), update_params)
					// NOTE: we have to fetch preferences because if we update session without
					// it then it effectively removes the preferences from the session
					.with(user::user_preferences::fetch())
					.exec()
					.await
			})
			.await?;
		Ok(User::from(updated_user_data))
	}
}

async fn update_preferences(
	client: &PrismaClient,
	preferences_id: String,
	input: UpdateUserPreferences,
) -> ApiResult<UserPreferences> {
	let updated_preferences = client
		.user_preferences()
		.update(
			user_preferences::id::equals(preferences_id),
			vec![
				user_preferences::locale::set(input.locale.to_owned()),
				user_preferences::library_layout_mode::set(
					input.library_layout_mode.to_owned(),
				),
				user_preferences::series_layout_mode::set(
					input.series_layout_mode.to_owned(),
				),
				user_preferences::app_theme::set(input.app_theme.to_owned()),
				user_preferences::show_query_indicator::set(input.show_query_indicator),
				user_preferences::enable_discord_presence::set(
					input.enable_discord_presence,
				),
			],
		)
		.exec()
		.await?;

	Ok(UserPreferences::from(updated_preferences))
}

#[derive(Deserialize, Type, ToSchema)]
pub struct CreateUser {
	pub username: String,
	pub password: String,
	#[serde(default)]
	pub permissions: Vec<UserPermission>,
	pub age_restriction: Option<AgeRestriction>,
}

#[utoipa::path(
	post,
	path = "/api/v1/users",
	tag = "user",
	request_body = CreateUser,
	responses(
		(status = 200, description = "Successfully created user", body = User),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Creates a new user.
async fn create_user(
	session: Session,
	State(ctx): State<AppState>,
	Json(input): Json<CreateUser>,
) -> ApiResult<Json<User>> {
	get_session_server_owner_user(&session)?;
	let db = ctx.get_db();

	let hashed_password = bcrypt::hash(input.password, get_hash_cost())?;

	// TODO: https://github.com/Brendonovich/prisma-client-rust/issues/44
	let created_user = db
		._transaction()
		.run(|client| async move {
			let permissions = input
				.permissions
				.into_iter()
				.filter_map(|p| {
					let p_str = p.to_string();
					if p_str.is_empty() {
						None
					} else {
						Some(p_str)
					}
				})
				.collect::<Vec<String>>();

			let created_user = client
				.user()
				.create(
					input.username.to_owned(),
					hashed_password,
					vec![
						user::is_server_owner::set(false),
						user::permissions::set(Some(permissions.join(","))),
					],
				)
				.exec()
				.await?;
			tracing::trace!(?created_user, "Created user");

			if let Some(ar) = input.age_restriction {
				let _age_restriction = client
					.age_restriction()
					.create(
						ar.age,
						user::id::equals(created_user.id.clone()),
						vec![age_restriction::restrict_on_unset::set(
							ar.restrict_on_unset,
						)],
					)
					.exec()
					.await?;
				tracing::trace!(?_age_restriction, "Created age restriction")
			}

			let _user_preferences = client
				.user_preferences()
				.create(vec![
					user_preferences::user::connect(user::id::equals(
						created_user.id.clone(),
					)),
					user_preferences::user_id::set(Some(created_user.id.clone())),
				])
				.exec()
				.await?;
			tracing::trace!(?_user_preferences, "Created user preferences");

			client
				.user()
				.find_unique(user::id::equals(created_user.id))
				.with(user::user_preferences::fetch())
				.with(user::age_restriction::fetch())
				.exec()
				.await
		})
		.await?
		.ok_or(ApiError::InternalServerError(
			"Failed to create user".to_string(),
		))?;
	tracing::trace!(final_user = ?created_user, "Final user result");

	Ok(Json(created_user.into()))
}

#[utoipa::path(
	put,
	path = "/api/v1/users/me",
	tag = "user",
	request_body = UpdateUser,
	responses(
		(status = 200, description = "Successfully updated user", body = User),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Updates the session user
async fn update_current_user(
	session: Session,
	State(ctx): State<AppState>,
	Json(input): Json<UpdateUser>,
) -> ApiResult<Json<User>> {
	let db = ctx.get_db();
	let user = get_session_user(&session)?;

	let updated_user = update_user(&user, db, user.id.clone(), input).await?;
	debug!(?updated_user, "Updated user");

	session
		.insert(SESSION_USER_KEY, updated_user.clone())
		.map_err(|e| {
			ApiError::InternalServerError(format!("Failed to update session: {}", e))
		})?;

	Ok(Json(updated_user))
}

#[derive(Debug, Clone, Deserialize, Type, ToSchema)]
pub struct UpdateUserPreferences {
	pub id: String,
	pub locale: String,
	pub library_layout_mode: String,
	pub series_layout_mode: String,
	pub collection_layout_mode: String,
	pub app_theme: String,
	pub show_query_indicator: bool,
	pub enable_discord_presence: bool,
}

#[utoipa::path(
	put,
	path = "/api/v1/users/me/preferences",
	tag = "user",
	request_body = UpdateUserPreferences,
	responses(
		(status = 200, description = "Successfully updated user preferences", body = UserPreferences),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Updates a user's preferences.
async fn update_current_user_preferences(
	session: Session,
	State(ctx): State<AppState>,
	Json(input): Json<UpdateUserPreferences>,
) -> ApiResult<Json<UserPreferences>> {
	let db = ctx.get_db();

	let user = get_session_user(&session)?;
	let user_preferences = user.user_preferences.clone().unwrap_or_default();

	trace!(user_id = ?user.id, ?user_preferences, updates = ?input, "Updating viewer's preferences");

	let updated_preferences = update_preferences(db, user_preferences.id, input).await?;
	debug!(?updated_preferences, "Updated user preferences");

	session
		.insert(
			"user",
			User {
				user_preferences: Some(updated_preferences.clone()),
				..user
			},
		)
		.map_err(|e| {
			ApiError::InternalServerError(format!("Failed to update session: {}", e))
		})?;

	Ok(Json(updated_preferences))
}

#[derive(Deserialize, Type, ToSchema)]
pub struct DeleteUser {
	pub hard_delete: Option<bool>,
}

#[utoipa::path(
	delete,
	path = "/api/v1/users/:id",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's id.", example = "1ab2c3d4")
	),
	responses(
		(status = 200, description = "Successfully deleted user", body = User),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 404, description = "User not found"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Deletes a user by ID.
async fn delete_user_by_id(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
	Json(input): Json<DeleteUser>,
) -> ApiResult<Json<User>> {
	let db = ctx.get_db();
	let user = get_session_server_owner_user(&session)?;

	if user.id == id {
		return Err(ApiError::BadRequest(
			"You cannot delete your own account.".into(),
		));
	}

	let hard_delete = input.hard_delete.unwrap_or(false);

	let deleted_user = if hard_delete {
		db.user().delete(user::id::equals(id.clone())).exec().await
	} else {
		db.user()
			.update(
				user::id::equals(id.clone()),
				vec![user::deleted_at::set(Some(Utc::now().into()))],
			)
			.exec()
			.await
	}?;

	debug!(?deleted_user, "Deleted user");

	Ok(Json(User::from(deleted_user)))
}

#[utoipa::path(
	get,
	path = "/api/v1/users/:id",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	responses(
		(status = 200, description = "Successfully fetched user", body = User),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 404, description = "User not found"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Gets a user by ID.
async fn get_user_by_id(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<Json<User>> {
	get_session_server_owner_user(&session)?;
	let db = ctx.get_db();
	let user_by_id = db
		.user()
		.find_unique(user::id::equals(id.clone()))
		.with(user::age_restriction::fetch())
		.exec()
		.await?;
	debug!(id, ?user_by_id, "Result of fetching user by id");

	if user_by_id.is_none() {
		return Err(ApiError::NotFound(format!("User with id {} not found", id)));
	}

	Ok(Json(User::from(user_by_id.unwrap())))
}

// TODO: pagination!
#[utoipa::path(
	get,
	path = "/api/v1/users/:id/login-activity",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	responses(
		(status = 200, description = "Successfully fetched user", body = Vec<LoginActivity>),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 404, description = "User not found"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn get_user_login_activity_by_id(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<Json<Vec<LoginActivity>>> {
	let user = get_session_user(&session)?;

	let client = ctx.get_db();

	if user.id != id && !user.is_server_owner {
		return Err(ApiError::Forbidden(String::from(
			"You cannot access this resource",
		)));
	}

	let user_activity = client
		.user_login_activity()
		.find_many(vec![user_login_activity::user_id::equals(id)])
		.order_by(user_login_activity::timestamp::order(Direction::Desc))
		.exec()
		.await?;

	Ok(Json(
		user_activity.into_iter().map(LoginActivity::from).collect(),
	))
}

#[utoipa::path(
	put,
	path = "/api/v1/users/:id",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	request_body = UpdateUser,
	responses(
		(status = 200, description = "Successfully updated user", body = User),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Updates a user by ID.
async fn update_user_handler(
	session: Session,
	State(ctx): State<AppState>,
	Path(id): Path<String>,
	Json(input): Json<UpdateUser>,
) -> ApiResult<Json<User>> {
	let db = ctx.get_db();
	let user = get_session_user(&session)?;

	if user.id != id && !user.is_server_owner {
		return Err(ApiError::forbidden_discreet());
	}

	let updated_user = update_user(&user, db, id.clone(), input).await?;
	debug!(?updated_user, "Updated user");

	if user.id == id {
		session
			.insert(SESSION_USER_KEY, updated_user.clone())
			.map_err(|e| {
				ApiError::InternalServerError(format!("Failed to update session: {}", e))
			})?;
	} else {
		// When a server owner updates another user, we need to delete all sessions for that user
		// because the user's permissions may have changed. This is a bit lazy but it works.
		db.session()
			.delete_many(vec![session::user_id::equals(id)])
			.exec()
			.await?;
	}

	Ok(Json(updated_user))
}

#[utoipa::path(
	delete,
	path = "/api/v1/users/:id/sessions",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	responses(
		(status = 200, description = "Successfully deleted user sessions"),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn delete_user_sessions(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<()> {
	get_session_server_owner_user(&session)?;

	let client = ctx.get_db();
	let removed_sessions = client
		.session()
		.delete_many(vec![session::user_id::equals(id)])
		.exec()
		.await?;
	tracing::trace!(?removed_sessions, "Removed sessions for user");

	Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateAccountLock {
	lock: bool,
}

#[utoipa::path(
	put,
	path = "/api/v1/users/:id/lock",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	request_body = UpdateAccountLock,
	responses(
		(status = 200, description = "Successfully updated user lock status", body = User),
		(status = 400, description = "You cannot lock your own account"),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
async fn update_user_lock_status(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
	Json(input): Json<UpdateAccountLock>,
) -> ApiResult<Json<User>> {
	let user = get_session_server_owner_user(&session)?;
	if user.id == id {
		return Err(ApiError::BadRequest(
			"You cannot lock your own account.".into(),
		));
	}

	let db = ctx.get_db();
	let updated_user = db
		.user()
		.update(
			user::id::equals(id.clone()),
			vec![user::is_locked::set(input.lock)],
		)
		.exec()
		.await?;

	if input.lock {
		// Delete all sessions for this user if they are being locked
		let removed_sessions = db
			.session()
			.delete_many(vec![session::user_id::equals(id)])
			.exec()
			.await?;
		tracing::trace!(?removed_sessions, "Removed sessions for locked user");
	}

	Ok(Json(User::from(updated_user)))
}

#[utoipa::path(
	get,
	path = "/api/v1/users/:id/preferences",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	responses(
		(status = 200, description = "Successfully fetched user preferences", body = UserPreferences),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 404, description = "User preferences not found"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Gets the user's preferences.
async fn get_user_preferences(
	Path(id): Path<String>,
	State(ctx): State<AppState>,
	session: Session,
) -> ApiResult<Json<UserPreferences>> {
	let db = ctx.get_db();
	let user = get_session_user(&session)?;

	if id != user.id {
		return Err(ApiError::forbidden_discreet());
	}

	let user_preferences = db
		.user_preferences()
		.find_unique(user_preferences::user_id::equals(id.clone()))
		.exec()
		.await?;
	debug!(id, ?user_preferences, "Fetched user preferences");

	if user_preferences.is_none() {
		return Err(ApiError::NotFound(format!(
			"User preferences with id {} not found",
			id
		)));
	}

	Ok(Json(UserPreferences::from(user_preferences.unwrap())))
}

// TODO: this is now a duplicate, do I need it? I think to remain RESTful, yes...
#[utoipa::path(
	put,
	path = "/api/v1/users/:id/preferences",
	tag = "user",
	params(
		("id" = String, Path, description = "The user's ID.", example = "1ab2c3d4")
	),
	request_body = UpdateUserPreferences,
	responses(
		(status = 200, description = "Successfully updated user preferences", body = UserPreferences),
		(status = 401, description = "Unauthorized"),
		(status = 403, description = "Forbidden"),
		(status = 500, description = "Internal server error"),
	)
)]
/// Updates a user's preferences.
async fn update_user_preferences(
	session: Session,
	State(ctx): State<AppState>,
	Path(id): Path<String>,
	Json(input): Json<UpdateUserPreferences>,
) -> ApiResult<Json<UserPreferences>> {
	trace!(?id, ?input, "Updating user preferences");
	let db = ctx.get_db();

	let user = get_session_user(&session)?;
	let user_preferences = user.user_preferences.clone().unwrap_or_default();

	if user_preferences.id != input.id {
		return Err(ApiError::forbidden_discreet());
	}

	let updated_preferences = update_preferences(db, user_preferences.id, input).await?;
	debug!(?updated_preferences, "Updated user preferences");

	session
		.insert(
			SESSION_USER_KEY,
			User {
				user_preferences: Some(updated_preferences.clone()),
				..user
			},
		)
		.map_err(|e| {
			ApiError::InternalServerError(format!("Failed to update session: {}", e))
		})?;

	Ok(Json(updated_preferences))
}
