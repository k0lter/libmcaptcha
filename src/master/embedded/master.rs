/*
 * mCaptcha - A proof of work based DoS protection system
 * Copyright © 2021 Aravinth Manivannan <realravinth@batsense.net>
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of the
 * License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Affero General Public License for more details.
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */
//! Embedded [Master] actor module that manages [Counter] actors
use std::collections::BTreeMap;
use std::time::Duration;

use actix::clock::sleep;
//use actix::clock::delay_for;
use actix::dev::*;
use log::info;
use tokio::sync::oneshot::channel;

use super::counter::Counter;
use crate::errors::*;
use crate::master::messages::{AddSite, AddVisitor, RemoveCaptcha, Rename};
use crate::master::Master as MasterTrait;

use super::counter::{GetCurrentVisitorCount, Stop};

/// This Actor manages the [Counter] actors.
/// A service can have several [Counter] actors with
/// varying [Defense][crate::defense::Defense] configurations
/// so a "master" actor is needed to manage them all
#[derive(Clone, Default)]
pub struct Master {
    sites: BTreeMap<String, (Option<()>, Addr<Counter>)>,
    gc: u64,
}

impl MasterTrait for Master {}

impl Master {
    /// add [Counter] actor to [Master]
    pub fn add_site(&mut self, addr: Addr<Counter>, id: String) {
        self.sites.insert(id, (None, addr));
    }

    /// create new master
    /// accepts a `u64` to configure garbage collection period
    pub fn new(gc: u64) -> Self {
        Master {
            sites: BTreeMap::new(),
            gc,
        }
    }

    /// get [Counter] actor from [Master]
    pub fn get_site(&mut self, id: &str) -> Option<Addr<Counter>> {
        let mut r = None;
        if let Some((read_val, addr)) = self.sites.get_mut(id) {
            r = Some(addr.clone());
            *read_val = Some(());
        };
        r
    }

    /// remvoes [Counter] actor from [Master]
    pub fn rm_site(&mut self, id: &str) -> Option<(Option<()>, Addr<Counter>)> {
        self.sites.remove(id)
    }

    /// renames [Counter] actor
    pub fn rename(&mut self, msg: Rename) {
        // If actor isn't present, it's okay to not throw an error
        // since actors are lazyily initialized and are cleaned up when inactive
        if let Some((_, counter)) = self.sites.remove(&msg.name) {
            self.add_site(counter, msg.rename_to);
        }
    }
}

impl Actor for Master {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let addr = ctx.address();
        let task = async move {
            addr.send(CleanUp).await.unwrap();
        }
        .into_actor(self);
        ctx.spawn(task);
    }
}

impl Handler<AddVisitor> for Master {
    type Result = MessageResult<AddVisitor>;

    fn handle(&mut self, m: AddVisitor, ctx: &mut Self::Context) -> Self::Result {
        let (tx, rx) = channel();
        match self.get_site(&m.0) {
            None => {
                let _ = tx.send(Ok(None));
            }
            Some(addr) => {
                let fut = async move {
                    match addr.send(super::counter::AddVisitor).await {
                        Ok(val) => {
                            let _ = tx.send(Ok(Some(val)));
                        }
                        Err(e) => {
                            let err: CaptchaError = e.into();
                            let _ = tx.send(Err(err));
                        }
                    }
                }
                .into_actor(self);
                ctx.spawn(fut);
            }
        }
        MessageResult(rx)
    }
}

impl Handler<Rename> for Master {
    type Result = MessageResult<Rename>;

    fn handle(&mut self, m: Rename, _ctx: &mut Self::Context) -> Self::Result {
        self.rename(m);
        let (tx, rx) = channel();
        let _ = tx.send(Ok(()));
        MessageResult(rx)
    }
}

/// Message to get an [Counter] actor from master
#[derive(Message)]
#[rtype(result = "Option<Addr<Counter>>")]
pub struct GetSite(pub String);

impl Handler<GetSite> for Master {
    type Result = MessageResult<GetSite>;

    fn handle(&mut self, m: GetSite, _ctx: &mut Self::Context) -> Self::Result {
        let addr = self.get_site(&m.0);
        match addr {
            None => MessageResult(None),
            Some(addr) => MessageResult(Some(addr)),
        }
    }
}

/// Message to clean up master of [Counter] actors with zero visitor count
#[derive(Message)]
#[rtype(result = "()")]
pub struct CleanUp;

impl Handler<CleanUp> for Master {
    type Result = ();

    fn handle(&mut self, _: CleanUp, ctx: &mut Self::Context) -> Self::Result {
        let sites = self.sites.clone();
        let gc = self.gc;
        let master = ctx.address();
        info!("init master actor cleanup up");
        let task = async move {
            for (id, (new, addr)) in sites.iter() {
                let visitor_count = addr.send(GetCurrentVisitorCount).await.unwrap();
                println!("{}", visitor_count);
                if visitor_count == 0 && new.is_some() {
                    addr.send(Stop).await.unwrap();
                    master.send(RemoveCaptcha(id.to_owned())).await.unwrap();
                    println!("cleaned up");
                }
            }

            let duration = Duration::new(gc, 0);
            sleep(duration).await;
            //delay_for(duration).await;
            master.send(CleanUp).await.unwrap();
        }
        .into_actor(self);
        ctx.spawn(task);
    }
}

impl Handler<RemoveCaptcha> for Master {
    type Result = MessageResult<RemoveCaptcha>;

    fn handle(&mut self, m: RemoveCaptcha, ctx: &mut Self::Context) -> Self::Result {
        let (tx, rx) = channel();
        if let Some((_, addr)) = self.rm_site(&m.0) {
            let fut = async move {
                //addr.send(Stop).await?;
                let res: CaptchaResult<()> = addr.send(Stop).await.map_err(|e| e.into());
                let _ = tx.send(res);
            }
            .into_actor(self);
            ctx.spawn(fut);
        } else {
            tx.send(Ok(())).unwrap();
        }
        MessageResult(rx)
    }
}

impl Handler<AddSite> for Master {
    type Result = MessageResult<AddSite>;

    fn handle(&mut self, m: AddSite, _ctx: &mut Self::Context) -> Self::Result {
        let (tx, rx) = channel();
        let counter: Counter = m.mcaptcha.into();
        let addr = counter.start();
        self.add_site(addr, m.id);
        tx.send(Ok(())).unwrap();
        MessageResult(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master::embedded::counter::tests::*;
    use crate::master::messages::AddSiteBuilder;
    use crate::master::messages::RenameBuilder;
    use crate::MCaptcha;

    #[actix_rt::test]
    async fn master_actor_works() {
        let addr = Master::new(1).start();

        let get_add_site_msg = |id: String, mcaptcha: MCaptcha| {
            AddSiteBuilder::default()
                .id(id)
                .mcaptcha(mcaptcha)
                .build()
                .unwrap()
        };

        let id = "yo";
        let msg = get_add_site_msg(id.into(), get_mcaptcha());

        addr.send(msg).await.unwrap();

        let mcaptcha_addr = addr.send(GetSite(id.into())).await.unwrap();
        assert!(mcaptcha_addr.is_some());

        let new_id = "yoyo";
        let rename = RenameBuilder::default()
            .name(id.into())
            .rename_to(new_id.into())
            .build()
            .unwrap();
        addr.send(rename).await.unwrap();
        let mcaptcha_addr = addr.send(GetSite(new_id.into())).await.unwrap();
        assert!(mcaptcha_addr.is_some());

        let addr_doesnt_exist = addr.send(GetSite("a".into())).await.unwrap();
        assert!(addr_doesnt_exist.is_none());

        let timer_expire = Duration::new(DURATION, 0);
        sleep(timer_expire).await;
        sleep(timer_expire).await;
        //        delay_for(timer_expire).await;
        //        delay_for(timer_expire).await;

        let mcaptcha_addr = addr.send(GetSite(new_id.into())).await.unwrap();
        assert_eq!(mcaptcha_addr, None);

        assert!(addr.send(RemoveCaptcha(new_id.into())).await.is_ok());
    }
}
