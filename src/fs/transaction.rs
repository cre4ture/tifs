use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::iter::FromIterator;
use std::ops::{Deref, DerefMut, Range};
use std::time::SystemTime;

use bytes::Bytes;
use bytestring::ByteString;
use fuser::{FileAttr, FileType};
use tikv_client::{Backoff, KvPair, RetryOptions, Transaction, TransactionClient, TransactionOptions};
use tracing::{debug, instrument, trace};
use tikv_client::transaction::Mutation;

use crate::fs::block;
use crate::fs::meta::StaticFsParameters;

use super::block::empty_block;
use super::dir::Directory;
use super::error::{FsError, Result};
use super::file_handler::FileHandler;
use super::hash_block::block_splitter::{BlockSplitterRead, BlockSplitterWrite};
use super::hash_block::helpers::UpdateIrregularBlock;
use super::hashed_block::HashBlockData;
use super::index::Index;
use super::inode::{self, BlockAddress, Inode, TiFsHash};
use super::key::{ScopedKeyBuilder, ScopedKeyKind, ROOT_INODE};
use super::meta::Meta;
use super::mode::{as_file_kind, as_file_perm, make_mode};
use super::reply::{DirItem, StatFs};
use super::tikv_fs::{DIR_PARENT, DIR_SELF};


pub const DEFAULT_REGION_BACKOFF: Backoff = Backoff::no_jitter_backoff(300, 1000, 100);
pub const OPTIMISTIC_BACKOFF: Backoff = Backoff::no_jitter_backoff(30, 500, 1000);
pub const PESSIMISTIC_BACKOFF: Backoff = Backoff::no_backoff();


pub struct Txn<'a> {
    key_builder: &'a ScopedKeyBuilder<'a>,
    txn: Transaction,
    hashed_blocks: bool,
    block_size: u64,
    max_blocks: Option<u64>,
    max_name_len: u32,
}

impl<'a> Txn<'a> {
    const INLINE_DATA_THRESHOLD_BASE: u64 = 1 << 4;

    fn inline_data_threshold(&self) -> u64 {
        self.block_size / Self::INLINE_DATA_THRESHOLD_BASE
    }

    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    fn check_space_left(&self, meta: &Meta) -> Result<()> {
        match meta.last_stat {
            Some(ref stat) if stat.bavail == 0 => {
                Err(FsError::NoSpaceLeft(stat.bsize as u64 * stat.blocks))
            }
            _ => Ok(()),
        }
    }

    pub async fn begin_optimistic(
        key_builder: &'a ScopedKeyBuilder<'a>,
        client: &TransactionClient,
        hashed_blocks: bool,
        block_size: usize,
        max_size: Option<u64>,
        max_name_len: u32,
    ) -> Result<Self> {
        let options = TransactionOptions::new_optimistic().use_async_commit();
        let options = options.retry_options(RetryOptions {
            region_backoff: DEFAULT_REGION_BACKOFF,
            lock_backoff: OPTIMISTIC_BACKOFF,
        });
        let txn: Transaction = client
            .begin_with_options(options)
            .await?;
        Ok(Txn {
            key_builder,
            txn,
            hashed_blocks,
            block_size: block_size as u64,
            max_blocks: max_size.map(|size| size / block_size as u64),
            max_name_len,
        })
    }

    pub async fn open(&mut self, ino: u64) -> Result<()> {
        let mut inode = self.read_inode(ino).await?;
        inode.opened_fh += 1;
        self.save_inode(&inode).await?;
        Ok(())
    }

    pub async fn close(&mut self, ino: u64) -> Result<()> {
        let mut inode = self.read_inode(ino).await?;
        inode.opened_fh -= 1;
        self.save_inode(&inode).await
    }

    pub async fn read(&mut self, ino: u64, handler: FileHandler, offset: i64, size: u32, update_atime: bool) -> Result<Vec<u8>> {
        let start = handler.cursor as i64 + offset;
        if start < 0 {
            return Err(FsError::InvalidOffset { ino, offset: start });
        }
        self.read_data(ino, start as u64, Some(size as u64), update_atime).await
    }

    pub async fn write(&mut self, ino: u64, handler: FileHandler, offset: i64, data: Bytes) -> Result<usize> {
        let start = handler.cursor as i64 + offset;
        if start < 0 {
            return Err(FsError::InvalidOffset { ino, offset: start });
        }

        self.write_data(ino, start as u64, data).await
    }

    pub async fn make_inode(
        &mut self,
        parent: u64,
        name: ByteString,
        mode: u32,
        gid: u32,
        uid: u32,
        rdev: u32,
    ) -> Result<Inode> {
        let mut meta = self
            .read_meta()
            .await?
            .unwrap_or_else(|| Meta::new(self.block_size as u64, StaticFsParameters{
                hashed_blocks: self.hashed_blocks
            }));
        self.check_space_left(&meta)?;
        let ino = meta.inode_next;
        meta.inode_next += 1;

        debug!("get ino({})", ino);
        self.save_meta(&meta).await?;

        let file_type = as_file_kind(mode);
        if parent >= ROOT_INODE {
            if self.get_index(parent, name.clone()).await?.is_some() {
                return Err(FsError::FileExist {
                    file: name.to_string(),
                });
            }
            self.set_index(parent, name.clone(), ino).await?;

            let mut dir = self.read_dir(parent).await?;
            debug!("read dir({:?})", &dir);

            dir.push(DirItem {
                ino,
                name: name.to_string(),
                typ: file_type,
            });

            self.save_dir(parent, &dir).await?;
            // TODO: update attributes of directory
        }

        let inode = FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: SystemTime::now(),
            kind: file_type,
            perm: as_file_perm(mode),
            nlink: 1,
            uid,
            gid,
            rdev,
            blksize: self.block_size as u32,
            flags: 0,
        }
        .into();

        debug!("made inode ({:?})", &inode);

        self.save_inode(&inode).await?;
        Ok(inode)
    }

    pub async fn get_index(&mut self, parent: u64, name: ByteString) -> Result<Option<u64>> {
        let key = self.key_builder.index(parent, &name);
        self.get(key)
            .await
            .map_err(FsError::from)
            .and_then(|value| {
                value
                    .map(|data| Ok(Index::deserialize(&data)?.ino))
                    .transpose()
            })
    }

    pub async fn set_index(&mut self, parent: u64, name: ByteString, ino: u64) -> Result<()> {
        let key = self.key_builder.index(parent, &name);
        let value = Index::new(ino).serialize()?;
        Ok(self.put(key, value).await?)
    }

    pub async fn remove_index(&mut self, parent: u64, name: ByteString) -> Result<()> {
        let key = self.key_builder.index(parent, &name);
        Ok(self.delete(key).await?)
    }

    pub async fn read_inode(&mut self, ino: u64) -> Result<Inode> {
        let key = self.key_builder.inode(ino);
        let value = self
            .get(key)
            .await?
            .ok_or(FsError::InodeNotFound { inode: ino })?;
        Ok(Inode::deserialize(&value)?)
    }

    pub async fn save_inode(&mut self, inode: &Inode) -> Result<()> {
        let key = self.key_builder.inode(inode.ino);

        if inode.nlink == 0 && inode.opened_fh == 0 {
            self.delete(key).await?;
        } else {
            self.put(key, inode.serialize()?).await?;
            debug!("save inode: {:?}", inode);
        }
        Ok(())
    }

    pub async fn remove_inode(&mut self, ino: u64) -> Result<()> {
        let key = self.key_builder.inode(ino);
        self.delete(key).await?;
        Ok(())
    }

    pub async fn read_meta(&mut self) -> Result<Option<Meta>> {
        let key = self.key_builder.meta();
        let opt_data = self.get(key).await?;
        opt_data.map(|data| Meta::deserialize(&data)).transpose()
    }

    pub async fn save_meta(&mut self, meta: &Meta) -> Result<()> {
        let key = self.key_builder.meta();
        self.put(key, meta.serialize()?).await?;
        Ok(())
    }

    async fn transfer_inline_data_to_block(&mut self, inode: &mut Inode) -> Result<()> {
        debug_assert!(inode.size <= self.inline_data_threshold());
        let key = self.key_builder.block(inode.ino, 0);
        let mut data = inode.inline_data.clone().unwrap();
        data.resize(self.block_size as usize, 0);
        self.put(key, data).await?;
        inode.inline_data = None;
        Ok(())
    }

    async fn write_inline_data(
        &mut self,
        inode: &mut Inode,
        start: u64,
        data: &[u8],
    ) -> Result<usize> {
        debug_assert!(inode.size <= self.inline_data_threshold());
        let size = data.len() as u64;
        debug_assert!(
            start + size <= self.inline_data_threshold(),
            "{} + {} > {}",
            start,
            size,
            self.inline_data_threshold()
        );

        let size = data.len();
        let start = start as usize;

        let mut inlined = inode.inline_data.take().unwrap_or_else(Vec::new);
        if start + size > inlined.len() {
            inlined.resize(start + size, 0);
        }
        inlined[start..start + size].copy_from_slice(data);

        inode.atime = SystemTime::now();
        inode.mtime = SystemTime::now();
        inode.ctime = SystemTime::now();
        inode.set_size(inlined.len() as u64, self.block_size);
        inode.inline_data = Some(inlined);
        self.save_inode(inode).await?;

        Ok(size)
    }

    async fn read_inline_data(
        &mut self,
        inode: &Inode,
        start: u64,
        size: u64,
    ) -> Result<Vec<u8>> {
        debug_assert!(inode.size <= self.inline_data_threshold());

        let start = start as usize;
        let size = size as usize;

        let inlined = inode.inline_data.as_ref().unwrap();
        debug_assert!(inode.size as usize == inlined.len());
        let mut data = vec![0; size];
        if inlined.len() > start {
            let to_copy = size.min(inlined.len() - start);
            data[..to_copy].copy_from_slice(&inlined[start..start + to_copy]);
        }
        Ok(data)
    }

    async fn hb_read_data(&mut self, ino: u64, start: u64, size: u64) -> Result<Vec<u8>> {

        let bs = BlockSplitterRead::new(self.block_size, start, size);
        let block_range = bs.first_block_index..bs.end_block_index;
        eprintln!("hb_read_data(ino: {ino}, start:{start}, size: {size}) - block_size: {}, blocks_count: {}, range: [{}..{}[", bs.block_size, bs.block_count, block_range.start, block_range.end);

        let block_hashes = self.hb_get_block_hash_list_by_block_range(ino, block_range.clone()).await?;
        eprintln!("block_hashes(count: {}): {:?}", block_hashes.len(), block_hashes);
        let block_hashes_set = HashSet::from_iter(block_hashes.values().cloned());
        let blocks_data = self.hb_get_block_data_by_hashes(&block_hashes_set).await?;

        let mut result = Vec::new();
        for block_index in block_range.clone() {

            let addr = BlockAddress{ino, index: block_index};
            let block_data = if let Some(block_hash) = block_hashes.get(&addr) {
                if let Some(data) = blocks_data.get(block_hash) {
                    data
                } else { &HashBlockData::Borrowed(&[]) }
            } else { &HashBlockData::Borrowed(&[]) };


            let (rd_start, rd_size) = match block_index {
                1 => (bs.first_block_read_offset as usize, bs.bytes_to_read_first_block as usize),
                _ => (0, (bs.size as usize - result.len()).min(bs.block_size as usize)),
            };

            if rd_start < block_data.len() {
                // do a copy, as some blocks might be used multiple times
                let rd_end = rd_start + rd_size;
                result.extend_from_slice(&block_data[rd_start..rd_end.min(block_data.len())]);
            }
        }

        Ok(result)
    }

    async fn read_data_traditional(&mut self, ino: u64, start: u64, size: u64) -> Result<Vec<u8>> {
        let target = start + size;
        let start_block = start / self.block_size as u64;
        let end_block = (target + self.block_size as u64 - 1) / self.block_size as u64;

        let block_range = self.key_builder.block_range(ino, start_block..end_block);
        let pairs = self
            .scan(
                block_range,
                (end_block - start_block) as u32,
            )
            .await?;

        let mut data = pairs
            .enumerate()
            .flat_map(|(i, pair)| {
                let key = if let Ok(ScopedKeyKind::Block { ino: _, block }) =
                    self.key_builder.parse(pair.key().into()).map(|x|x.key_type)
                {
                    block
                } else {
                    unreachable!("the keys from scanning should be always valid block keys")
                };
                let value = pair.into_value();
                (start_block as usize + i..key as usize)
                    .map(|_| empty_block(self.block_size))
                    .chain(vec![value])
            })
            .enumerate()
            .fold(
                Vec::with_capacity(
                    ((end_block - start_block) * self.block_size - start % self.block_size)
                        as usize,
                ),
                |mut data, (i, value)| {
                    let mut slice = value.as_slice();
                    if i == 0 {
                        slice = &slice[(start % self.block_size) as usize..]
                    }

                    data.extend_from_slice(slice);
                    data
                },
            );

        data.resize(size as usize, 0);
        Ok(data)
    }

    pub async fn read_data(
        &mut self,
        ino: u64,
        start: u64,
        chunk_size: Option<u64>,
        update_atime: bool,
    ) -> Result<Vec<u8>> {
        let attr = self.read_inode(ino).await?;
        if start >= attr.size {
            return Ok(Vec::new());
        }

        let max_size = attr.size - start;
        let size = chunk_size.unwrap_or(max_size).min(max_size);

        if update_atime {
            let mut attr = attr.clone();
            attr.atime = SystemTime::now();
            self.save_inode(&attr).await?;
        }

        if attr.inline_data.is_some() {
            return self.read_inline_data(&attr, start, size).await;
        }

        if self.hashed_blocks {
            self.hb_read_data(ino, start, size).await
        } else {
            self.read_data_traditional(ino, start, size).await
        }
    }

    pub async fn clear_data(&mut self, ino: u64) -> Result<u64> {
        let mut attr = self.read_inode(ino).await?;
        let end_block = (attr.size + self.block_size - 1) / self.block_size;

        for block in 0..end_block {
            let key = self.key_builder.block(ino, block);
            self.delete(key).await?;
        }

        let clear_size = attr.size;
        attr.size = 0;
        attr.atime = SystemTime::now();
        self.save_inode(&attr).await?;
        Ok(clear_size)
    }

    pub async fn write_blocks_traditional(&mut self, ino: u64, start: u64, data: &Bytes) -> Result<()> {

        let mut block_index = start / self.block_size;
        let start_key = self.key_builder.block(ino, block_index);
        let start_index = (start % self.block_size) as usize;

        let first_block_size = self.block_size as usize - start_index;

        let (first_block, mut rest) = data.split_at(first_block_size.min(data.len()));

        let mut start_value = self
            .get(start_key)
            .await?
            .unwrap_or_else(|| empty_block(self.block_size));

        start_value[start_index..start_index + first_block.len()].copy_from_slice(first_block);

        self.put(start_key, start_value).await?;

        while !rest.is_empty() {
            block_index += 1;
            let key = self.key_builder.block(ino, block_index);
            let (curent_block, current_rest) =
                rest.split_at((self.block_size as usize).min(rest.len()));
            let mut value = curent_block.to_vec();
            if value.len() < self.block_size as usize {
                let mut last_value = self
                    .get(key)
                    .await?
                    .unwrap_or_else(|| empty_block(self.block_size));
                last_value[..value.len()].copy_from_slice(&value);
                value = last_value;
            }
            self.put(key, value).await?;
            rest = current_rest;
        }

        Ok(())
    }

    pub async fn hb_get_block_hash_list_by_block_range(&mut self, ino: u64, block_range: Range<u64>) -> Result<HashMap<BlockAddress, inode::Hash>>
    {
        let range = self.key_builder.block_hash_range(ino, block_range.clone());
        let iter = self
            .scan(
                range,
                block_range.count() as u32,
            )
            .await?;
        Ok(iter.filter_map(|pair| {
            let Some(key) = self.key_builder.parse_key_block_address(pair.key().into()) else {
                tracing::error!("failed parsing block address from response 1");
                return None;
            };
            let hash = if pair.value().len() >= blake3::OUT_LEN {
                let data: &[u8; blake3::OUT_LEN] = pair.value()[0..blake3::OUT_LEN].try_into().unwrap();
                inode::Hash::from_bytes(data.to_owned())
            } else {
                tracing::error!("failed parsing hash value from response 2");
                return None;
            };
            Some((key, hash))
        }).collect())
    }

    // pub async fn hb_get_block_data_by_block_index(&mut self, inode: &Inode, index: u64) -> Result<HashedBlock> {
    //     let Some(start_hash_vec) = &inode.block_hashes else {
    //         return Ok(self.hb_new_block());
    //     };

    //     let maybe_hash = start_hash_vec.get(index as usize);
    //     let Some(Some(hash)) = maybe_hash else {
    //         return Ok(self.hb_new_block());
    //     };

    //     self.hb_get_block_data_by_hashes(&[hash])
    //         .await?.into_iter().next().ok_or_else(FsError::BlockNotFound { inode: inode.ino, block: index })
    // }

    pub async fn hb_get_block_data_by_hashes(&mut self, hash_list: &HashSet<TiFsHash>) -> Result<HashMap<inode::Hash, HashBlockData<'_>>>
    {
        let key = hash_list.iter().map(|h| self.key_builder.hashed_block(&h)).collect::<Vec<_>>();
        let data_list = self.txn.batch_get(key).await?;
        Ok(data_list.into_iter().filter_map(|pair| {
            let Some(hash) = self.key_builder.parse_key_hashed_block(pair.key().into()) else {
                tracing::error!("failed parsing hash from response!");
                return None;
            };
            Some((hash, Cow::Owned(pair.into_value())))
        }).collect())
    }

    // pub async fn hb_irregular_update_block_data(
    //     &mut self,
    //     hash: inode::Hash,
    //     start_write_pos: usize,
    //     block_write_data: &[u8],
    //     mutations: &mut Vec<Mutation>) -> Result<()> {

    //     let existing_block_data = self.hb_get_block_data_by_block_index(&mut inode, block_index).await?;
    //     if let Some(existing_hash) = &existing_block_data.hash {
    //         mutations.push(Mutation::Delete(self.key_builder.hashed_block(existing_hash)))
    //     }
    //     existing_block_data.update_data_range(start_write_pos, block_write_data);
    //     let new_hash = existing_block_data.update_hash();

    //     mutations.push(Mutation::Put(self.key_builder.hashed_block(new_hash), new_hash.data));
    //     Ok(())
    // }

    pub async fn hb_write_data(&mut self, inode: &mut Inode, start: u64, data: &Bytes) -> Result<()> {

        let bs = BlockSplitterWrite::new(self.block_size, start, &data);
        let block_range = bs.get_range();
        eprintln!("hb_write_data(ino: {}, start:{}, size: {}) - block_size: {}, blocks_count: {}, range: [{}..{}[", inode.ino, start, data.len(), bs.block_size, block_range.count(), block_range.start, block_range.end);

        let hash_list_prev = self.hb_get_block_hash_list_by_block_range(inode.ino, block_range.clone()).await?;

        let mut pre_data_hash_request = HashSet::<inode::Hash>::new();
        let first_data_handler = UpdateIrregularBlock::get_and_add_original_block_hash(
            inode.ino, bs.first_data, bs.first_data_start_position, &hash_list_prev, &mut pre_data_hash_request
        );
        let last_data_handler = UpdateIrregularBlock::get_and_add_original_block_hash(
            inode.ino, bs.last_data, 0, &hash_list_prev, &mut pre_data_hash_request
        );

        let pre_data = self.hb_get_block_data_by_hashes(&pre_data_hash_request).await?;
        let mut new_blocks = HashMap::new();
        let mut new_block_hashes = HashMap::new();

        first_data_handler.get_and_modify_block_and_publish_hash(&pre_data, &mut new_blocks, &mut new_block_hashes);
        last_data_handler.get_and_modify_block_and_publish_hash(&pre_data, &mut new_blocks, &mut new_block_hashes);

        for (index, chunk) in bs.mid_data.data.chunks(self.block_size as usize).enumerate() {
            let hash = blake3::hash(chunk);
            new_blocks.insert(hash, Cow::Borrowed(chunk));
            new_block_hashes.insert(BlockAddress{ino: inode.ino, index: bs.mid_data.block_index + index as u64}, hash);
        }

        let exists_keys_request = new_blocks.keys().map(|k| self.key_builder.hashed_block_exists(k)).collect::<Vec<_>>();
        let exists_keys_response = self.batch_get(exists_keys_request).await?.collect::<Vec<_>>();
        for KvPair(key, _) in exists_keys_response.into_iter() {
            let key = (&key).into();
            let hash = self.key_builder.parse_key_hashed_block(key).ok_or(FsError::UnknownError("failed parsing hash from response".into()))?;
            new_blocks.remove(&hash);
        }

        let mut mutations = Vec::<Mutation>::new();
        // upload new blocks:
        for (k, new_block) in new_blocks {
            mutations.push(Mutation::Put(self.key_builder.hashed_block(&k).into(), new_block.into()));
            mutations.push(Mutation::Put(self.key_builder.hashed_block_exists(&k).into(), vec![]));
        }

        // remove outdated blocks:
        // TODO!

        // filter out unchanged blocks:
        for (address, prev_block_hash) in hash_list_prev.iter() {
            if let Some(new_block_hash) = new_block_hashes.get(&address) {
                if prev_block_hash == new_block_hash {
                    new_block_hashes.remove(&address);
                }
            }
        }

        // write new block mapping:
        for (k, new_hash) in new_block_hashes {
            mutations.push(Mutation::Put(self.key_builder.block_hash(k).into(), new_hash.as_bytes().to_vec()));
        }

        // execute all
        self.batch_mutate(mutations).await?;

        Ok(())
    }

    #[instrument(skip(self, data))]
    pub async fn write_data(&mut self, ino: u64, start: u64, data: Bytes) -> Result<usize> {
        let write_start = SystemTime::now();
        debug!("write data at ({})[{}]", ino, start);
        let meta = self.read_meta().await?.unwrap();
        self.check_space_left(&meta)?;

        let mut inode = self.read_inode(ino).await?;
        let size = data.len();
        let target = start + size as u64;

        if inode.inline_data.is_some() && target > self.inline_data_threshold() {
            self.transfer_inline_data_to_block(&mut inode).await?;
        }

        if (inode.inline_data.is_some() || inode.size == 0)
            && target <= self.inline_data_threshold()
        {
            return self.write_inline_data(&mut inode, start, &data).await;
        }

        if self.hashed_blocks {
            self.hb_write_data(&mut inode, start, &data).await?;
        } else {
            self.write_blocks_traditional(ino, start, &data).await?;
        }

        inode.atime = SystemTime::now();
        inode.mtime = SystemTime::now();
        inode.ctime = SystemTime::now();
        inode.set_size(inode.size.max(target), self.block_size);
        self.save_inode(&inode).await?;
        trace!("write data: {}", String::from_utf8_lossy(&data));
        debug!(
            "write {} bytes in {}ms",
            data.len(),
            write_start.elapsed().unwrap().as_millis()
        );
        Ok(size)
    }

    pub async fn write_link(&mut self, inode: &mut Inode, data: Bytes) -> Result<usize> {
        debug_assert!(inode.file_attr.kind == FileType::Symlink);
        inode.inline_data = None;
        inode.set_size(0, self.block_size);
        self.write_inline_data(inode, 0, &data).await
    }

    pub async fn read_link(&mut self, ino: u64, update_atime: bool) -> Result<Vec<u8>> {
        let inode = self.read_inode(ino).await?;
        debug_assert!(inode.file_attr.kind == FileType::Symlink);
        let size = inode.size;
        if update_atime {
            let mut inode = inode.clone();
            inode.atime = SystemTime::now();
            self.save_inode(&inode).await?;
        }
        self.read_inline_data(&inode, 0, size).await
    }

    pub async fn link(&mut self, ino: u64, newparent: u64, newname: ByteString) -> Result<Inode> {
        if let Some(old_ino) = self.get_index(newparent, newname.clone()).await? {
            let inode = self.read_inode(old_ino).await?;
            match inode.kind {
                FileType::Directory => self.rmdir(newparent, newname.clone()).await?,
                _ => self.unlink(newparent, newname.clone()).await?,
            }
        }
        self.set_index(newparent, newname.clone(), ino).await?;

        let mut inode = self.read_inode(ino).await?;
        let mut dir = self.read_dir(newparent).await?;

        dir.push(DirItem {
            ino,
            name: newname.to_string(),
            typ: inode.kind,
        });

        self.save_dir(newparent, &dir).await?;
        inode.nlink += 1;
        inode.ctime = SystemTime::now();
        self.save_inode(&inode).await?;
        Ok(inode)
    }

    pub async fn unlink(&mut self, parent: u64, name: ByteString) -> Result<()> {
        match self.get_index(parent, name.clone()).await? {
            None => Err(FsError::FileNotFound {
                file: name.to_string(),
            }),
            Some(ino) => {
                self.remove_index(parent, name.clone()).await?;
                let parent_dir = self.read_dir(parent).await?;
                let new_parent_dir: Directory = parent_dir
                    .into_iter()
                    .filter(|item| item.name != *name)
                    .collect();
                self.save_dir(parent, &new_parent_dir).await?;

                let mut inode = self.read_inode(ino).await?;
                inode.nlink -= 1;
                inode.ctime = SystemTime::now();
                self.save_inode(&inode).await?;
                Ok(())
            }
        }
    }

    pub async fn rmdir(&mut self, parent: u64, name: ByteString) -> Result<()> {
        match self.get_index(parent, name.clone()).await? {
            None => Err(FsError::FileNotFound {
                file: name.to_string(),
            }),
            Some(ino) => {
                if self
                    .read_dir(ino)
                    .await?
                    .iter()
                    .any(|i| DIR_SELF != i.name && DIR_PARENT != i.name)
                {
                    let name_str = name.to_string();
                    debug!("dir({}) not empty", &name_str);
                    return Err(FsError::DirNotEmpty { dir: name_str });
                }

                self.unlink(ino, DIR_SELF).await?;
                self.unlink(ino, DIR_PARENT).await?;
                self.unlink(parent, name).await
            }
        }
    }

    pub async fn lookup(&mut self, parent: u64, name: ByteString) -> Result<u64> {
        self.get_index(parent, name.clone())
            .await?
            .ok_or_else(|| FsError::FileNotFound {
                file: name.to_string(),
            })
    }

    pub async fn fallocate(&mut self, inode: &mut Inode, offset: i64, length: i64) -> Result<()> {
        let target_size = (offset + length) as u64;
        if target_size <= inode.size {
            return Ok(());
        }

        if inode.inline_data.is_some() {
            if target_size <= self.inline_data_threshold() {
                let original_size = inode.size;
                let data = vec![0; (target_size - original_size) as usize];
                self.write_inline_data(inode, original_size, &data).await?;
                return Ok(());
            } else {
                self.transfer_inline_data_to_block(inode).await?;
            }
        }

        inode.set_size(target_size, self.block_size);
        inode.mtime = SystemTime::now();
        self.save_inode(inode).await?;
        Ok(())
    }

    pub async fn mkdir(
        &mut self,
        parent: u64,
        name: ByteString,
        mode: u32,
        gid: u32,
        uid: u32,
    ) -> Result<Inode> {
        let dir_mode = make_mode(FileType::Directory, mode as _);
        let mut inode = self.make_inode(parent, name, dir_mode, gid, uid, 0).await?;
        inode.perm = mode as _;
        self.save_inode(&inode).await?;
        self.save_dir(inode.ino, &Directory::new()).await?;
        self.link(inode.ino, inode.ino, DIR_SELF).await?;
        if parent >= ROOT_INODE {
            self.link(parent, inode.ino, DIR_PARENT).await?;
        }
        self.read_inode(inode.ino).await
    }

    pub async fn read_dir(&mut self, ino: u64) -> Result<Directory> {
        let key = self.key_builder.block(ino, 0);
        let data = self
            .get(key)
            .await?
            .ok_or(FsError::BlockNotFound {
                inode: ino,
                block: 0,
            })?;
        trace!("read data: {}", String::from_utf8_lossy(&data));
        super::dir::decode(&data)
    }

    pub async fn save_dir(&mut self, ino: u64, dir: &[DirItem]) -> Result<Inode> {
        let data = super::dir::encode(dir)?;
        let mut inode = self.read_inode(ino).await?;
        inode.set_size(data.len() as u64, self.block_size);
        inode.atime = SystemTime::now();
        inode.mtime = SystemTime::now();
        inode.ctime = SystemTime::now();
        self.save_inode(&inode).await?;
        let key = self.key_builder.block(ino, 0);
        self.put(key, data).await?;
        Ok(inode)
    }

    pub async fn statfs(&mut self) -> Result<StatFs> {
        let bsize = self.block_size as u32;
        let mut meta = self
            .read_meta()
            .await?
            .expect("meta should not be none after fs initialized");
        let next_inode = meta.inode_next;
        let range = self.key_builder.inode_range(ROOT_INODE..next_inode);
        let (used_blocks, files) = self
            .scan(
                range,
                (next_inode - ROOT_INODE) as u32,
            )
            .await?
            .map(|pair| Inode::deserialize(pair.value()))
            .try_fold((0, 0), |(blocks, files), inode| {
                Ok::<_, FsError>((blocks + inode?.blocks, files + 1))
            })?;
        let ffree = std::u64::MAX - next_inode;
        let bfree = match self.max_blocks {
            Some(max_blocks) if max_blocks > used_blocks => max_blocks - used_blocks,
            Some(_) => 0,
            None => std::u64::MAX,
        };
        let blocks = match self.max_blocks {
            Some(max_blocks) => max_blocks,
            None => used_blocks,
        };

        let stat = StatFs::new(
            blocks,
            bfree,
            bfree,
            files,
            ffree,
            bsize,
            self.max_name_len,
            0,
        );
        trace!("statfs: {:?}", stat);
        meta.last_stat = Some(stat.clone());
        self.save_meta(&meta).await?;
        Ok(stat)
    }
}

impl<'a> Deref for Txn<'a> {
    type Target = Transaction;

    fn deref(&self) -> &Self::Target {
        &self.txn
    }
}

impl<'a> DerefMut for Txn<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.txn
    }
}
