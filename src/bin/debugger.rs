use std::fmt::Debug;
use std::io::{stdin, stdout, BufRead, BufReader, Write};

use anyhow::{anyhow, Result};
use clap::{crate_version, App, Arg};
use tikv_client::{Transaction, TransactionClient};
use tracing_subscriber::EnvFilter;

use tifs::ScopedKey;

#[async_std::main]
async fn main() -> Result<()> {
    let matches = App::new("TiFS Debugger")
        .version(crate_version!())
        .author("Hexi Lee")
        .arg(
            Arg::with_name("pd")
                .long("pd-endpoints")
                .multiple(true)
                .value_name("ENDPOINTS")
                .default_value("127.0.0.1:2379")
                .help("set all pd endpoints of the tikv cluster")
                .takes_value(true),
        )
        .get_matches();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .unwrap();

    let endpoints: Vec<&str> = matches
        .values_of("pd")
        .unwrap_or_default()
        .to_owned()
        .collect();

    let console = Console::construct(endpoints).await?;

    loop {
        match console.interact().await {
            Ok(true) => break Ok(()),
            Err(err) => eprintln!("{}", err),
            _ => continue,
        }
    }
}

struct Console {
    pd_endpoints: Vec<String>,
    client: TransactionClient,
}

impl Console {
    async fn construct<S>(pd_endpoints: Vec<S>) -> Result<Self>
    where
        S: Clone + Debug + Into<String>,
    {
        let client = TransactionClient::new_with_config(pd_endpoints.clone(), Default::default())
            .await
            .map_err(|err| anyhow!("{}", err))?;
        Ok(Self {
            client,
            pd_endpoints: pd_endpoints.into_iter().map(Into::into).collect(),
        })
    }

    async fn interact(&self) -> Result<bool> {
        let mut txn = self.client.begin_optimistic().await?;
        match self.interact_with_txn(&mut txn).await {
            Ok(exit) => {
                txn.commit().await?;
                Ok(exit)
            }
            Err(err) => {
                txn.rollback().await?;
                Err(err)
            }
        }
    }

    async fn interact_with_txn(&self, txn: &mut Transaction) -> Result<bool> {
        print!("{:?}> ", &self.pd_endpoints);
        stdout().flush()?;

        let mut buffer = String::new();
        BufReader::new(stdin()).read_line(&mut buffer)?;
        let commands: Vec<&str> = buffer.split(" ").map(|seg| seg.trim()).collect();
        if commands.len() == 0 {
            return Ok(false);
        }

        match commands[0] {
            "exit" => return Ok(true),
            "get_block" => self.get_block(txn, &commands[1..]).await?,
            cmd => return Err(anyhow!("unknow command `{}`", cmd)),
        }

        Ok(false)
    }

    async fn get_block(&self, txn: &mut Transaction, args: &[&str]) -> Result<()> {
        if args.len() != 2 {
            return Err(anyhow!("invalid arguments `{:?}`", args));
        }
        match txn
            .get(ScopedKey::new(args[0].parse()?, args[1].parse()?))
            .await?
        {
            Some(value) => println!("{:?}", value),
            None => println!("Not Found"),
        }
        Ok(())
    }
}