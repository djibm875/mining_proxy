use anyhow::{bail, Result};

use std::sync::Arc;
use tracing::{debug, info};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, WriteHalf},
    select,
    sync::RwLockReadGuard,
    time,
};

use crate::{
    client::*,
    protocol::{
        ethjson::{EthServerRoot, EthServerRootObject},
        CLIENT_LOGIN, CLIENT_SUBMITWORK,
    },
    state::Worker,
    util::{config::Settings, is_fee_random},
};
use crate::{
    protocol::ethjson::{
        login, new_eth_get_work, new_eth_submit_hashrate, new_eth_submit_work,
        EthServer, EthServerRootObjectJsonRpc,
    },
    DEVELOP_FEE, DEVELOP_WORKER_NAME,
};

pub async fn handle_stream<R, W, PR, PW>(
    worker: &mut Worker,
    worker_r: tokio::io::BufReader<tokio::io::ReadHalf<R>>,
    mut worker_w: WriteHalf<W>,
    pool_r: tokio::io::BufReader<tokio::io::ReadHalf<PR>>,
    mut pool_w: WriteHalf<PW>, proxy: Arc<Proxy>, is_encrypted: bool,
) -> Result<()>
where
    R: AsyncRead,
    W: AsyncWrite,
    PR: AsyncRead,
    PW: AsyncWrite,
{
    let mut worker_name: String = String::new();
    let mut eth_server_result = EthServerRoot {
        id: 0,
        jsonrpc: "2.0".into(),
        result: true,
    };

    let mut job_rpc = EthServerRootObjectJsonRpc {
        id: 0,
        jsonrpc: "2.0".into(),
        result: vec![],
    };

    let mut fee_job: Vec<String> = Vec::new();
    let mut dev_fee_job: Vec<String> = Vec::new();

    //最后一次发送的rpc_id
    let mut rpc_id = 0;

    // 如果任务时重复的，就等待一次下次发送
    let mut dev_send_idx = 0;
    let mut job_idx = 0;

    //let mut total_send_idx = 0;

    // 包装为封包格式。
    let mut pool_lines = pool_r.lines();
    let mut worker_lines;
    let mut send_job = Vec::new();

    if is_encrypted {
        worker_lines = worker_r.split(SPLIT);
    } else {
        worker_lines = worker_r.split(b'\n');
    }

    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha20Rng::from_entropy();
    let send_time = rand::Rng::gen_range(&mut rng, 1..360) as u64;
    let workers_queue = proxy.worker_tx.clone();
    let sleep = time::sleep(tokio::time::Duration::from_secs(send_time));
    tokio::pin!(sleep);

    let mut chan = proxy.chan.subscribe();
    let mut dev_chan = proxy.dev_chan.subscribe();

    let tx = proxy.tx.clone();
    let dev_tx = proxy.dev_tx.clone();

    // 当前Job高度。
    let mut job_hight = 0;
    // 欠了几个job
    // let mut dev_fee_idx = 0;
    // let mut fee_idx = 0;
    // let mut idx = 0;

    let mut wait_job: VecDeque<Vec<String>> = VecDeque::new();
    let mut wait_dev_job: VecDeque<Vec<String>> = VecDeque::new();

    let config: Settings;
    {
        let rconfig = RwLockReadGuard::map(proxy.config.read().await, |s| s);
        config = rconfig.clone();
    }

    loop {
        select! {
            res = worker_lines.next_segment() => {
                let start = std::time::Instant::now();
                let buf_bytes = seagment_unwrap(&mut pool_w,res,&worker_name).await?;

                //每次获取一次config. 有更新的话就使用新的了
                //let config: Settings;
                // {
                //     let rconfig = RwLockReadGuard::map(proxy.config.read().await, |s| s);
                //     config = rconfig.clone();
                // }

                // if is_encrypted {
                //     let key = Vec::from_hex(config.key.clone()).unwrap();
                //     let iv = Vec::from_hex(config.iv.clone()).unwrap();
                //     let cipher = Cipher::aes_256_cbc();

                //     buf_bytes = match base64::decode(&buf_bytes[..]) {
                //         Ok(buffer) => buffer,
                //         Err(e) => {
                //             tracing::error!("{}",e);
                //             match pool_w.shutdown().await  {
                //                 Ok(_) => {},
                //                 Err(_) => {
                //                     tracing::error!("Error Shutdown Socket {:?}",e);
                //                 },
                //             };
                //             bail!("解密矿机请求失败{}",e);
                //         },
                //     };

                //     buf_bytes = match decrypt(
                //         cipher,
                //         &key,
                //         Some(&iv),
                //         &buf_bytes[..]) {
                //             Ok(s) => s,
                //             Err(e) => {
                //                 tracing::warn!("加密报文解密失败");
                //                 match pool_w.shutdown().await  {
                //                     Ok(_) => {},
                //                     Err(e) => {
                //                         tracing::error!("Error Shutdown Socket {:?}",e);
                //                     },
                //                 };
                //                 bail!("解密矿机请求失败{}",e);
                //         },
                //     };
                // }

                // if is_encrypted {
                //     let key = config.key.clone();
                //     let iv = config.iv.clone();
                //     buf_bytes = match base64::decode(&buf_bytes[..]) {
                //         Ok(buffer) => buffer,
                //         Err(e) => {
                //             tracing::error!("{}",e);
                //             match pool_w.shutdown().await  {
                //                 Ok(_) => {},
                //                 Err(_) => {
                //                     tracing::error!("Error Shutdown Socket {:?}",e);
                //                 },
                //             };
                //             bail!("解密矿机请求失败{}",e);
                //         },
                //     };
                //     //GenericArray::from(&buf_bytes[..]);
                //     //let cipher = Aes128::new(&cipherkey);
                //     let key = Key::from_slice(key.as_bytes());
                //     let cipher = Aes256Gcm::new(key);

                //     let nonce = Nonce::from_slice(iv.as_bytes()); // 96-bits; unique per message

                //     // let ciphertext = cipher.encrypt(nonce, b"plaintext message".as_ref())
                //     //     .expect("encryption failure!"); // NOTE: handle this error to avoid panics!

                //     buf_bytes = match cipher.decrypt(nonce, buf_bytes.as_ref()){
                //         Ok(s) => s,
                //         Err(e) => {
                //             tracing::warn!("加密报文解密失败");
                //             match pool_w.shutdown().await  {
                //                 Ok(_) => {},
                //                 Err(e) => {
                //                     tracing::error!("Error Shutdown Socket {:?}",e);
                //                 },
                //             };
                //             bail!("解密矿机请求失败{}",e);
                //         },
                //     };
                // }

                #[cfg(debug_assertions)]
                debug!("0:  矿机 -> 矿池 {} #{:?}", worker_name, String::from_utf8(buf_bytes.clone()).unwrap());

                let buf_bytes = buf_bytes.split(|c| *c == b'\n');
                for buffer in buf_bytes {
                    if buffer.is_empty() {
                        continue;
                    }

                    if let Some(mut json_rpc) = parse(buffer) {
                        #[cfg(debug_assertions)]
                        info!("接受矿工: {} 提交 RPC {:?}",worker.worker_name,json_rpc);
                        rpc_id = json_rpc.get_id();
                        let res = match json_rpc.get_method().as_str() {
                            "eth_submitLogin" => {
                                eth_server_result.id = rpc_id;
                                login(worker,&mut pool_w,&mut json_rpc,&mut worker_name,&config).await?;
                                write_rpc(is_encrypted,&mut worker_w,&eth_server_result,&worker_name).await?;
                                Ok(())
                            },
                            "eth_submitWork" => {
                                eth_server_result.id = rpc_id;
                                if let Some(job_id) = json_rpc.get_job_id() {
                                    #[cfg(debug_assertions)]
                                    debug!("0 :  收到提交工作量 {} #{:?}",worker_name, json_rpc);
                                    let mut json_rpc = Box::new(EthClientWorkerObject{ id: json_rpc.get_id(), method: json_rpc.get_method(), params: json_rpc.get_params(), worker: worker.worker_name.clone()});

                                    if dev_fee_job.contains(&job_id) {
                                        json_rpc.set_worker_name(&DEVELOP_WORKER_NAME.to_string());
                                        dev_tx.send(json_rpc).await?;
                                    } else if fee_job.contains(&job_id) {
                                        json_rpc.set_worker_name(&config.share_name.clone());
                                        tx.send(json_rpc).await?;
                                        worker.fee_share_index_add();
                                        worker.fee_share_accept();
                                    } else {
                                        worker.share_index_add();
                                        new_eth_submit_work(worker,&mut pool_w,&mut worker_w,&mut json_rpc,&worker_name,&config).await?;
                                    }

                                    write_rpc(is_encrypted,&mut worker_w,&eth_server_result,&worker_name).await?;
                                    Ok(())
                                } else {
                                    pool_w.shutdown().await?;
                                    worker_w.shutdown().await?;
                                    bail!("非法攻击");
                                }
                            },
                            "eth_submitHashrate" => {
                                eth_server_result.id = rpc_id;
                                let mut hash = json_rpc.get_submit_hashrate();
                                hash = (hash as f64 * (config.hash_rate as f32 / 100.0) as f64) as u64;
                                json_rpc.set_submit_hashrate(format!("0x{:x}", hash));
                                new_eth_submit_hashrate(worker,&mut pool_w,&mut json_rpc,&worker_name).await?;
                                write_rpc(is_encrypted,&mut worker_w,&eth_server_result,&worker_name).await?;
                                Ok(())
                            },
                            "eth_getWork" => {
                                new_eth_get_work(&mut pool_w,&mut json_rpc,&worker_name).await?;
                                // eth_server_result.id = rpc_id;
                                // write_rpc(is_encrypted,&mut worker_w,&eth_server_result,&worker_name).await?;
                                Ok(())
                            },
                            "mining.subscribe" =>{ //GMiner
                                new_eth_get_work(&mut pool_w,&mut json_rpc,&worker_name).await?;
                                eth_server_result.id = rpc_id;
                                write_rpc(is_encrypted,&mut worker_w,&eth_server_result,&worker_name).await?;
                                Ok(())
                            }
                            _ => {
                                // tracing::warn!("Not found method {:?}",json_rpc);
                                // eth_server_result.id = rpc_id;
                                // write_to_socket_byte(&mut pool_w,buffer.to_vec(),&mut worker_name).await?;
                                pool_w.shutdown().await?;
                                worker_w.shutdown().await?;
                                return Ok(());
                            },
                        };

                        if res.is_err() {
                            tracing::warn!("写入任务错误: {:?}",res);
                            return res;
                        }
                    } else {
                        tracing::warn!("协议解析错误: {:?}",buffer);
                        bail!("未知的协议{}",buf_parse_to_string(&mut worker_w,&buffer).await?);
                    }
                }
                #[cfg(debug_assertions)]
                info!("接受矿工: {} 提交处理时间{:?}",worker.worker_name,start.elapsed());
            },
            res = pool_lines.next_line() => {
                let buffer = lines_unwrap(res,&worker_name,"矿池").await?;
                #[cfg(debug_assertions)]
                debug!("1 :  矿池 -> 矿机 {} #{:?}",worker_name, buffer);

                let buffer: Vec<_> = buffer.split("\n").collect();

                for buf in buffer {
                    if buf.is_empty() {
                        continue;
                    }

                    if let Ok(rpc) = serde_json::from_str::<EthServerRootObject>(buf) {
                        // 增加索引
                        worker.send_job()?;
                        if is_fee_random(*DEVELOP_FEE) {
                            #[cfg(debug_assertions)]
                            debug!("进入开发者抽水回合");

                            if let Some(job_res) = wait_dev_job.pop_back() {
                            //if let Ok(job_res) =  dev_chan.try_recv() {
                                {
                                    job_rpc.result = job_res.clone();
                                    let hi = job_rpc.get_hight();
                                    if hi != 0 {
                                        if job_hight < hi {
                                            #[cfg(debug_assertions)]
                                            debug!(worker=?worker,hight=?hi,"开发者抽水任务 高度已经改变.");
                                            wait_dev_job.clear();
                                            wait_job.clear();
                                            job_hight = hi;
                                            continue;
                                        } else if job_hight > hi {
                                            // 陈旧任务.
                                            #[cfg(debug_assertions)]
                                            debug!(worker=?worker,hight=?hi,job=?job_rpc,"抽水获取到 陈旧的任务。不再分配");
                                            continue;
                                        } else {
                                            #[cfg(debug_assertions)]
                                            debug!(worker=?worker,hight=?hi,job=?job_rpc,"已分配开发者抽水任务");
                                            worker.send_develop_job()?;
                                            #[cfg(debug_assertions)]
                                            debug!("获取开发者抽水任务成功 {:?}",&job_res);
                                            job_rpc.result = job_res;
                                            let job_id = job_rpc.get_job_id().unwrap();
                                            dev_fee_job.push(job_id.clone());
                                            #[cfg(debug_assertions)]
                                            debug!("{} 发送开发者任务 #{:?}",worker_name, job_rpc);
                                            write_rpc(is_encrypted,&mut worker_w,&job_rpc,&worker_name).await?;
                                            continue;
                                        }
                                    }
                                }

                                worker.send_develop_job()?;
                                #[cfg(debug_assertions)]
                                debug!("获取开发者抽水任务成功 {:?}",&job_res);
                                job_rpc.result = job_res;
                                let job_id = job_rpc.get_job_id().unwrap();
                                dev_fee_job.push(job_id.clone());
                                #[cfg(debug_assertions)]
                                debug!("{} 发送开发者任务 #{:?}",worker_name, job_rpc);
                                write_rpc(is_encrypted,&mut worker_w,&job_rpc,&worker_name).await?;
                                continue;
                            }
                        } else if is_fee_random(config.share_rate.into()) {
                            #[cfg(debug_assertions)]
                            debug!("进入普通抽水回合");
                            if let Some(job_res) = wait_job.pop_back() {
                            //if let Ok(job_res) =  chan.try_recv() {
                                
                                job_rpc.result = job_res.clone();
                                let hi = job_rpc.get_hight();
                                if hi != 0 {
                                    if job_hight < hi {
                                        #[cfg(debug_assertions)]
                                        debug!(worker=?worker,hight=?hi,"抽水任务 高度已经改变.");
                                        wait_dev_job.clear();
                                        wait_job.clear();
                                        job_hight = hi;
                                        continue;
                                    } else if job_hight > hi {
                                        // 陈旧任务.
                                        #[cfg(debug_assertions)]
                                        debug!(worker=?worker,hight=?hi,job=?job_rpc,"抽水获取到 陈旧的任务。不再分配");
                                        continue;
                                    } else {
                                        worker.send_fee_job()?;
                                        job_rpc.result = job_res;
                                        let job_id = job_rpc.get_job_id().unwrap();
                                        fee_job.push(job_id.clone());
                                        #[cfg(debug_assertions)]
                                        debug!("{} 发送抽水任务 #{:?}",worker_name, job_rpc);
                                        write_rpc(is_encrypted,&mut worker_w,&job_rpc,&worker_name).await?;
                                        continue;
                                    }
                                }


                                worker.send_fee_job()?;
                                job_rpc.result = job_res;
                                let job_id = job_rpc.get_job_id().unwrap();
                                fee_job.push(job_id.clone());
                                #[cfg(debug_assertions)]
                                debug!("{} 发送抽水任务 #{:?}",worker_name, job_rpc);
                                write_rpc(is_encrypted,&mut worker_w,&job_rpc,&worker_name).await?;
                                continue;
                            }
                        }


                        //TODO Job diff 处理。如果接收到的任务已经过期。就跳过此任务分配。等待下次任务分配。
                        job_rpc.result = rpc.result;
                        let hi = job_rpc.get_hight();
                        if hi != 0 {
                            if job_hight < hi {
                                #[cfg(debug_assertions)]
                                debug!(worker=?worker,hight=?hi,"普通任务 高度已经改变.");
                                wait_dev_job.clear();
                                wait_job.clear();
                                job_hight = hi;
                                continue;
                            } else if job_hight > hi {
                                // 陈旧任务.
                                debug!(worker=?worker,hight=?hi,job=?job_rpc,"陈旧的任务。不再分配");
                                continue;
                            }
                        }

                        let job_id = job_rpc.get_job_id().unwrap();
                        send_job.push(job_id);
                        #[cfg(debug_assertions)]
                        debug!("{} 发送普通任务 #{:?}",worker_name, job_rpc);
                        write_rpc(is_encrypted,&mut worker_w,&job_rpc,&worker_name).await?;

                    } else if let Ok(result_rpc) = serde_json::from_str::<EthServer>(&buf) {
                        if result_rpc.id == CLIENT_LOGIN {
                            worker.logind();
                        } else if result_rpc.id == CLIENT_SUBMITWORK && result_rpc.result {
                            worker.share_accept();
                        } else if result_rpc.id == CLIENT_SUBMITWORK {
                            worker.share_reject();
                        }
                    }
                }
            },
            Ok(job_res) = dev_chan.recv() => {
                job_rpc.result = job_res.clone();
                let hi = job_rpc.get_hight();
                if hi != 0 && job_hight < hi {
                    #[cfg(debug_assertions)]
                    debug!(worker=?worker,hight=?hi,"开发者 高度已经改变.");
                    wait_dev_job.clear();
                    wait_job.clear();
                    job_hight = hi;
                }
                wait_dev_job.push_back(job_res);
            },Ok(job_res) = chan.recv() => {
                job_rpc.result = job_res.clone();
                let hi = job_rpc.get_hight();
                if hi != 0 && job_hight < hi {
                    #[cfg(debug_assertions)]
                    debug!(worker=?worker,hight=?hi,"中转 高度已经改变.");
                    wait_dev_job.clear();
                    wait_job.clear();
                    job_hight = hi;
                }
                wait_job.push_back(job_res);
            },
            () = &mut sleep  => {
                match workers_queue.send(worker.clone()) {
                    Ok(_) => {},
                    Err(_) => {
                        tracing::warn!("发送矿工状态失败");
                    },
                };
                sleep.as_mut().reset(time::Instant::now() + time::Duration::from_secs(send_time));
            },
        }
    }
}
