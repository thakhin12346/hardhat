import { spawn } from "node:child_process";

import type { Client as ClientT } from "undici";

import { HardhatNetworkChainsConfig } from "../../../../src/types/config";
import { defaultHardhatNetworkParams } from "../../../../src/internal/core/config/default-config";
import { BackwardsCompatibilityProviderAdapter } from "../../../../src/internal/core/providers/backwards-compatibility";
import { HttpProvider } from "../../../../src/internal/core/providers/http";
import { JsonRpcServer } from "../../../../src/internal/hardhat-network/jsonrpc/server";
import {
  ForkConfig,
  MempoolOrder,
} from "../../../../src/internal/hardhat-network/provider/node-types";
import { HardhatNetworkProvider } from "../../../../src/internal/hardhat-network/provider/provider";
import {
  EIP1193Provider,
  EthereumProvider,
  HardhatNetworkMempoolConfig,
  HardhatNetworkMiningConfig,
} from "../../../../src/types";

import { FakeModulesLogger } from "./fakeLogger";
import {
  DEFAULT_ACCOUNTS,
  DEFAULT_ALLOW_UNLIMITED_CONTRACT_SIZE,
  DEFAULT_BLOCK_GAS_LIMIT,
  DEFAULT_CHAIN_ID,
  DEFAULT_HARDFORK,
  DEFAULT_MINING_CONFIG,
  DEFAULT_NETWORK_ID,
  DEFAULT_MEMPOOL_CONFIG,
  DEFAULT_USE_JSON_RPC,
  DEFAULT_USE_RETHNET_CLI,
} from "./providers";

declare module "mocha" {
  interface Context {
    logger: FakeModulesLogger;
    provider: EthereumProvider;
    hardhatNetworkProvider: HardhatNetworkProvider;
    server?: JsonRpcServer;
    serverInfo?: { address: string; port: number };
  }
}

export interface UseProviderOptions {
  useJsonRpc?: boolean;
  useRethnetCli?: boolean;
  loggerEnabled?: boolean;
  forkConfig?: ForkConfig;
  mining?: HardhatNetworkMiningConfig;
  hardfork?: string;
  chainId?: number;
  networkId?: number;
  blockGasLimit?: bigint;
  accounts?: Array<{ privateKey: string; balance: bigint }>;
  allowUnlimitedContractSize?: boolean;
  allowBlocksWithSameTimestamp?: boolean;
  initialBaseFeePerGas?: bigint;
  mempool?: HardhatNetworkMempoolConfig;
  coinbase?: string;
  chains?: HardhatNetworkChainsConfig;
  forkBlockNumber?: number;
}

function getHttpClientProvider(url: string, name = "rethnet"): EIP1193Provider {
  const { Client } = require("undici") as { Client: typeof ClientT };

  const dispatcher = new Client(url, {
    keepAliveTimeout: 10,
    keepAliveMaxTimeout: 10,
  });

  return new HttpProvider(
    url,
    name,
    {},
    20000,
    dispatcher
  );
};

export function useProvider({
  useJsonRpc = DEFAULT_USE_JSON_RPC,
  useRethnetCli = DEFAULT_USE_RETHNET_CLI,
  loggerEnabled = true,
  forkConfig,
  mining = DEFAULT_MINING_CONFIG,
  hardfork = DEFAULT_HARDFORK,
  chainId = DEFAULT_CHAIN_ID,
  networkId = DEFAULT_NETWORK_ID,
  blockGasLimit = DEFAULT_BLOCK_GAS_LIMIT,
  accounts = DEFAULT_ACCOUNTS,
  allowUnlimitedContractSize = DEFAULT_ALLOW_UNLIMITED_CONTRACT_SIZE,
  allowBlocksWithSameTimestamp = false,
  initialBaseFeePerGas,
  mempool = DEFAULT_MEMPOOL_CONFIG,
  coinbase,
  chains = defaultHardhatNetworkParams.chains,
}: UseProviderOptions = {}) {
  beforeEach("Initialize provider", async function () {
    this.logger = new FakeModulesLogger(loggerEnabled);
    this.hardhatNetworkProvider = new HardhatNetworkProvider(
      {
        hardfork,
        chainId,
        networkId,
        blockGasLimit: Number(blockGasLimit),
        initialBaseFeePerGas:
          initialBaseFeePerGas === undefined
            ? undefined
            : Number(initialBaseFeePerGas),
        minGasPrice: 0n,
        throwOnTransactionFailures: true,
        throwOnCallFailures: true,
        automine: mining.auto,
        intervalMining: mining.interval,
        mempoolOrder: mempool.order as MempoolOrder,
        chains,
        genesisAccounts: accounts,
        allowUnlimitedContractSize,
        forkConfig,
        coinbase,
        allowBlocksWithSameTimestamp,
      },
      this.logger
    );
    this.provider = new BackwardsCompatibilityProviderAdapter(
      this.hardhatNetworkProvider
    );

    if (useJsonRpc) {
      this.server = new JsonRpcServer({
        port: 0,
        hostname: "127.0.0.1",
        provider: this.provider,
      });
      this.serverInfo = await this.server.listen();

      this.provider = new BackwardsCompatibilityProviderAdapter(
        this.server.getProvider()
      );
    }

    if (useRethnetCli) {
      console.log("spawning");
      this.rethnetProcess = spawn("rethnet", ["node"]);
      await new Promise(resolve => setTimeout(resolve, 250));
      this.rethnetProcess.stdout.on("data", (data: any) => {
        console.log(`rethnet subprocess ${this.rethnetProcess.pid}: ${data}`);
      });
      this.rethnetProcess.on("error", (err: Error) => {
        /* what's the best way to handle errors? this way doesn't seem perfect,
         * because it seems to induce a "double error": done() called multiple
         * times in hook <Eth module Rethnet provider "before each" hook:
         * Initialize provider in "Rethnet provider"> of file
         * /home/gene/dev/nomiclabs/rethnet/packages/hardhat-core/test/internal/hardhat-network/provider/modules/eth/methods/getBalance.ts;
         * in addition, done() received error: Error: Rethnet executable not found */
        if (err.message.includes("ENOENT")) {
          throw new Error("Rethnet executable not found");
        } else {
          throw new Error(`Rethnet subprocess error: ${err}`);
        }
      });
      this.provider = new BackwardsCompatibilityProviderAdapter(
        getHttpClientProvider("http://127.0.0.1:8545")
      );
    }
  });

  afterEach("Remove provider", async function () {
    // These two deletes are unsafe, but we use this properties
    // in very locally and are ok with the risk.
    // To make this safe the properties should be optional, which
    // would be really uncomfortable for testing.
    delete (this as any).provider;
    delete (this as any).hardhatNetworkProvider;

    if (this.server !== undefined) {
      // close server and fail if it takes too long
      const beforeClose = Date.now();
      await this.server.close();
      const afterClose = Date.now();
      const elapsedTime = afterClose - beforeClose;
      if (elapsedTime > 1500) {
        throw new Error(
          `Closing the server took more than 1 second (${elapsedTime}ms), which can lead to incredibly slow tests. Please fix it.`
        );
      }

      delete this.server;
      delete this.serverInfo;
    }

    if (this.rethnetProcess !== undefined) {
      this.rethnetProcess.kill();
    }
  });
}
