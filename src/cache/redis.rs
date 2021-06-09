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
//! Cache implementation that uses Redis
use crate::redis::mcaptcha_redis::MCaptchaRedis;
use crate::redis::RedisConfig;
use std::collections::HashMap;

use actix::prelude::*;

use super::messages::*;
use super::AddChallenge;
use super::Save;
use crate::errors::*;

pub struct RedisCache(MCaptchaRedis);

#[derive(Clone, Default)]
/// cache datastructure implementing [Save]
pub struct HashCache {
    difficulty_map: HashMap<String, CachedPoWConfig>,
    result_map: HashMap<String, String>,
}

impl RedisCache {
    /// Get new [MCaptchaRedis]. Use this when executing commands that are
    /// only supported by mCaptcha Redis module. Internally, when object
    /// is created, checks are performed to check if the module is loaded and if
    /// the required commands are available
    pub async fn new(redis: RedisConfig) -> CaptchaResult<Self> {
        let m = MCaptchaRedis::new(redis).await?;
        Ok(Self(m))
    }

    // save [PoWConfig] to cache
    async fn save_pow_config(&mut self, config: CachePoW) -> CaptchaResult<()> {
        let challenge = config.string;
        let payload: AddChallenge = AddChallenge {
            challenge: config.string,
            difficulty: config.difficulty_factor as usize,
            duration: config.duration,
        };

        let payload = serde_json::to_string(&payload).unwrap();

        // {MCAPTCHA_NAME}:difficulty_map:challenge (difficulty_factor -> duration) EX duration

        // TODO use hashmap

        self.difficulty_map.insert(challenge, config);
        Ok(())
    }

    // retrive [PoWConfig] from cache. Deletes config post retrival
    fn retrive_pow_config(&mut self, string: String) -> CaptchaResult<Option<CachedPoWConfig>> {
        if let Some(difficulty_factor) = self.remove_pow_config(&string) {
            Ok(Some(difficulty_factor.to_owned()))
        } else {
            Ok(None)
        }
    }

    // delete [PoWConfig] from cache
    fn remove_pow_config(&mut self, string: &str) -> Option<CachedPoWConfig> {
        self.difficulty_map.remove(string)
    }

    // save captcha result
    async fn save_captcha_result(&mut self, res: CacheResult) -> CaptchaResult<()> {
        self.result_map.insert(res.token, res.key);

        // {MCAPTCHA_NAME}:result_map:token 0 EX duration
        Ok(())
    }

    // verify captcha result
    fn verify_captcha_result(&mut self, challenge: VerifyCaptchaResult) -> CaptchaResult<bool> {
        if let Some(captcha_id) = self.remove_cache_result(&challenge.token) {
            if captcha_id == challenge.key {
                return Ok(true);
            } else {
                return Ok(false);
            }
        } else {
            Ok(false)
        }
    }

    // delete cache result
    fn remove_cache_result(&mut self, string: &str) -> Option<String> {
        self.result_map.remove(string)
    }
}

impl Save for HashCache {}

impl Actor for HashCache {
    type Context = Context<Self>;
}

/// cache a PoWConfig
impl Handler<CachePoW> for HashCache {
    type Result = MessageResult<CachePoW>;
    fn handle(&mut self, msg: CachePoW, ctx: &mut Self::Context) -> Self::Result {
        //use actix::clock::sleep;
        use actix::clock::delay_for;
        use std::time::Duration;

        let addr = ctx.address();
        let del_msg = DeletePoW(msg.string.clone());

        let duration: Duration = Duration::new(msg.duration.clone(), 0);
        let wait_for = async move {
            //sleep(duration).await;
            delay_for(duration).await;
            addr.send(del_msg).await.unwrap().unwrap();
        }
        .into_actor(self);
        ctx.spawn(wait_for);

        MessageResult(self.save_pow_config(msg))
    }
}

/// Delte a PoWConfig
impl Handler<DeletePoW> for HashCache {
    type Result = MessageResult<DeletePoW>;
    fn handle(&mut self, msg: DeletePoW, _ctx: &mut Self::Context) -> Self::Result {
        self.remove_pow_config(&msg.0);
        MessageResult(Ok(()))
    }
}

/// Retrive PoW difficulty_factor for a PoW string
impl Handler<RetrivePoW> for HashCache {
    type Result = MessageResult<RetrivePoW>;
    fn handle(&mut self, msg: RetrivePoW, _ctx: &mut Self::Context) -> Self::Result {
        MessageResult(self.retrive_pow_config(msg.0))
    }
}

/// cache PoW result
impl Handler<CacheResult> for HashCache {
    type Result = MessageResult<CacheResult>;
    fn handle(&mut self, msg: CacheResult, ctx: &mut Self::Context) -> Self::Result {
        //use actix::clock::sleep;
        use actix::clock::delay_for;
        use std::time::Duration;

        let addr = ctx.address();
        let del_msg = DeleteCaptchaResult {
            token: msg.token.clone(),
        };

        let duration: Duration = Duration::new(msg.duration.clone(), 0);
        let wait_for = async move {
            //sleep(duration).await;
            delay_for(duration).await;
            addr.send(del_msg).await.unwrap().unwrap();
        }
        .into_actor(self);
        ctx.spawn(wait_for);

        MessageResult(self.save_captcha_result(msg))
    }
}

/// Delte a PoWConfig
impl Handler<DeleteCaptchaResult> for HashCache {
    type Result = MessageResult<DeleteCaptchaResult>;
    fn handle(&mut self, msg: DeleteCaptchaResult, _ctx: &mut Self::Context) -> Self::Result {
        self.remove_cache_result(&msg.token);
        MessageResult(Ok(()))
    }
}

/// Retrive PoW difficulty_factor for a PoW string
impl Handler<VerifyCaptchaResult> for HashCache {
    type Result = MessageResult<VerifyCaptchaResult>;
    fn handle(&mut self, msg: VerifyCaptchaResult, _ctx: &mut Self::Context) -> Self::Result {
        // MessageResult(self.retrive(msg.0))
        MessageResult(self.verify_captcha_result(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master::AddVisitorResult;
    use crate::pow::PoWConfig;

    //   async fn sleep(time: u64) {
    //       //use actix::clock::sleep;
    //       use actix::clock::delay_for;
    //       use std::time::Duration;

    //       let duration: Duration = Duration::new(time, 0);
    //       //sleep(duration).await;
    //       delay_for(duration).await;
    //   }

    #[actix_rt::test]
    async fn hashcache_pow_cache_works() {
        use actix::clock::delay_for;
        use actix::clock::Duration;

        const DIFFICULTY_FACTOR: u32 = 54;
        const DURATION: u64 = 5;
        const KEY: &str = "mcaptchakey";
        let addr = HashCache::default().start();
        let pow: PoWConfig = PoWConfig::new(DIFFICULTY_FACTOR, KEY.into()); //salt is dummy here
        let visitor_result = AddVisitorResult {
            difficulty_factor: DIFFICULTY_FACTOR,
            duration: DURATION,
        };
        let string = pow.string.clone();

        let msg = CachePoWBuilder::default()
            .string(pow.string.clone())
            .difficulty_factor(DIFFICULTY_FACTOR)
            .duration(visitor_result.duration)
            .key(KEY.into())
            .build()
            .unwrap();

        addr.send(msg).await.unwrap().unwrap();

        let cache_difficulty_factor = addr
            .send(RetrivePoW(string.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            DIFFICULTY_FACTOR,
            cache_difficulty_factor.unwrap().difficulty_factor
        );

        let duration: Duration = Duration::new(5, 0);
        //sleep(DURATION + DURATION).await;
        delay_for(duration + duration).await;

        let expired_string = addr.send(RetrivePoW(string)).await.unwrap().unwrap();
        assert_eq!(None, expired_string);
    }

    #[actix_rt::test]
    async fn hashcache_result_cache_works() {
        use actix::clock::delay_for;
        use actix::clock::Duration;

        const DURATION: u64 = 5;
        const KEY: &str = "a";
        const RES: &str = "b";
        let addr = HashCache::default().start();
        // send value to cache
        // send another value to cache for auto delete
        // verify_captcha_result
        // delete
        // wait for timeout and verify_captcha_result against second value

        let add_cache = CacheResult {
            key: KEY.into(),
            token: RES.into(),
            duration: DURATION,
        };

        addr.send(add_cache).await.unwrap().unwrap();

        let verify_msg = VerifyCaptchaResult {
            key: KEY.into(),
            token: RES.into(),
        };

        assert!(addr.send(verify_msg.clone()).await.unwrap().unwrap());
        // duplicate
        assert!(!addr.send(verify_msg).await.unwrap().unwrap());

        let verify_msg = VerifyCaptchaResult {
            key: "cz".into(),
            token: RES.into(),
        };
        assert!(!addr.send(verify_msg).await.unwrap().unwrap());

        let duration: Duration = Duration::new(5, 0);
        delay_for(duration + duration).await;

        let verify_msg = VerifyCaptchaResult {
            key: KEY.into(),
            token: RES.into(),
        };
        assert!(!addr.send(verify_msg).await.unwrap().unwrap());
    }
}
