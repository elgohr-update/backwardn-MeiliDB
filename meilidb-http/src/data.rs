use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use heed::types::{SerdeBincode, Str};
use log::error;
use meilidb_core::{Database, Error as MError, MResult};
use sysinfo::Pid;

use crate::option::Opt;
use crate::routes::index::index_update_callback;

const LAST_UPDATE_KEY: &str = "last-update";

type SerdeDatetime = SerdeBincode<DateTime<Utc>>;

#[derive(Clone)]
pub struct Data {
    inner: Arc<DataInner>,
}

impl Deref for Data {
    type Target = DataInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Clone)]
pub struct DataInner {
    pub db: Arc<Database>,
    pub db_path: String,
    pub api_key: Option<String>,
    pub server_pid: Pid,
}

impl DataInner {
    pub fn is_indexing(&self, reader: &heed::RoTxn, index: &str) -> MResult<Option<bool>> {
        match self.db.open_index(&index) {
            Some(index) => index.current_update_id(&reader).map(|u| Some(u.is_some())),
            None => Ok(None),
        }
    }

    pub fn last_update(&self, reader: &heed::RoTxn) -> MResult<Option<DateTime<Utc>>> {
        match self
            .db
            .common_store()
            .get::<Str, SerdeDatetime>(reader, LAST_UPDATE_KEY)?
        {
            Some(datetime) => Ok(Some(datetime)),
            None => Ok(None),
        }
    }

    pub fn set_last_update(&self, writer: &mut heed::RwTxn) -> MResult<()> {
        self.db
            .common_store()
            .put::<Str, SerdeDatetime>(writer, LAST_UPDATE_KEY, &Utc::now())
            .map_err(Into::into)
    }

    pub fn compute_stats(&self, writer: &mut heed::RwTxn, index_uid: &str) -> MResult<()> {
        let index = match self.db.open_index(&index_uid) {
            Some(index) => index,
            None => {
                error!("Impossible to retrieve index {}", index_uid);
                return Ok(());
            }
        };

        let schema = match index.main.schema(&writer)? {
            Some(schema) => schema,
            None => return Ok(()),
        };

        let all_documents_fields = index
            .documents_fields_counts
            .all_documents_fields_counts(&writer)?;

        // count fields frequencies
        let mut fields_frequency = HashMap::<_, usize>::new();
        for result in all_documents_fields {
            let (_, attr, _) = result?;
            *fields_frequency.entry(attr).or_default() += 1;
        }

        // convert attributes to their names
        let frequency: HashMap<_, _> = fields_frequency
            .into_iter()
            .map(|(a, c)| (schema.attribute_name(a).to_owned(), c))
            .collect();

        index
            .main
            .put_fields_frequency(writer, &frequency)
            .map_err(MError::Zlmdb)
    }
}

impl Data {
    pub fn new(opt: Opt) -> Data {
        let db_path = opt.db_path.clone();
        let api_key = opt.api_key.clone();
        let server_pid = sysinfo::get_current_pid().unwrap();

        let db = Arc::new(Database::open_or_create(opt.db_path.clone()).unwrap());

        let inner_data = DataInner {
            db: db.clone(),
            db_path,
            api_key,
            server_pid,
        };

        let data = Data {
            inner: Arc::new(inner_data),
        };

        let callback_context = data.clone();
        db.set_update_callback(Box::new(move |index_uid, status| {
            index_update_callback(&index_uid, &callback_context, status);
        }));

        data
    }
}
