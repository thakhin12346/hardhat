use std::fmt::Debug;

use rethnet_eth::{Address, Bytes, U256};
use revm::{
    interpreter::{
        instruction_result::SuccessOrHalt, opcode, return_revert, CallInputs, CreateInputs, Gas,
        InstructionResult, Interpreter,
    },
    primitives::{AccountInfo, Bytecode, ExecutionResult, Output},
    Database, EVMData, Inspector,
};

/// Message
pub enum TraceMessage {
    Before(BeforeMessage),
    Step(Step),
    After(ExecutionResult),
}

/// A trace for an EVM call.
#[derive(Default)]
pub struct Trace {
    // /// The individual steps of the call
    // pub steps: Vec<Step>,
    /// Messages
    pub messages: Vec<TraceMessage>,
    /// The return value of the call
    pub return_value: Bytes,
    gas: Option<Gas>,
}

/// Temporary before message type for handling traces
#[derive(Clone)]
pub struct BeforeMessage {
    /// Call depth
    pub depth: usize,
    /// Callee
    pub to: Option<Address>,
    /// Input data
    pub data: Bytes,
    /// Value
    pub value: U256,
    /// Code address
    pub code_address: Option<Address>,
    /// Bytecode
    pub code: Option<Bytecode>,
}

/// A single EVM step.
pub struct Step {
    /// The program counter
    pub pc: u64,
    /// The call depth
    pub depth: u64,
    // /// The executed op code
    // pub opcode: u8,
    // /// The amount of gas that was used by the step
    // pub gas_cost: u64,
    // /// The amount of gas that was refunded by the step
    // pub gas_refunded: i64,
    // /// The contract being executed
    // pub contract: AccountInfo,
    // /// The address of the contract
    // pub contract_address: Address,
}

impl Trace {
    /// Adds a before message
    pub fn add_before(&mut self, message: BeforeMessage) {
        self.messages.push(TraceMessage::Before(message));
    }

    /// Adds a result message
    pub fn add_after(&mut self, result: ExecutionResult) {
        self.messages.push(TraceMessage::After(result));
    }

    /// Adds a VM step to the trace.
    pub fn add_step(
        &mut self,
        depth: u64,
        pc: usize,
        _opcode: u8,
        _gas: &Gas,
        _contract: &AccountInfo,
        _contract_address: &Address,
    ) {
        self.messages.push(TraceMessage::Step(Step {
            pc: pc as u64,
            depth,
            // opcode,
            // contract: contract.clone(),
            // contract_address: *contract_address,
        }));
    }
}

/// Object that gathers trace information during EVM execution and can be turned into a trace upon completion.
#[derive(Default)]
pub struct TraceCollector {
    trace: Trace,
    pending_before: Option<BeforeMessage>,
}

impl TraceCollector {
    /// Converts the [`Tracer`] into its [`Trace`].
    pub fn into_trace(self) -> Trace {
        self.trace
    }

    fn validate_before_message(&mut self) {
        if let Some(message) = self.pending_before.take() {
            self.trace.add_before(message);
        }
    }
}

impl<D> Inspector<D> for TraceCollector
where
    D: Database,
    D::Error: Debug,
{
    fn call(
        &mut self,
        data: &mut EVMData<'_, D>,
        inputs: &mut CallInputs,
        _is_static: bool,
    ) -> (InstructionResult, Gas, rethnet_eth::Bytes) {
        self.validate_before_message();

        let code = data
            .journaled_state
            .state
            .get(&inputs.context.code_address)
            .map(|account| {
                if let Some(code) = &account.info.code {
                    code.clone()
                } else {
                    data.db.code_by_hash(account.info.code_hash).unwrap()
                }
            })
            .unwrap_or_else(|| {
                let account = data.db.basic(inputs.context.code_address).unwrap().unwrap();
                account
                    .code
                    .unwrap_or_else(|| data.db.code_by_hash(account.code_hash).unwrap())
            });

        self.pending_before = Some(BeforeMessage {
            depth: data.journaled_state.depth,
            to: Some(inputs.context.address),
            data: inputs.input.clone(),
            value: inputs.transfer.value,
            code_address: Some(inputs.context.code_address),
            code: Some(code),
        });

        (InstructionResult::Continue, Gas::new(0), Bytes::default())
    }

    fn call_end(
        &mut self,
        data: &mut EVMData<'_, D>,
        _inputs: &CallInputs,
        remaining_gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        match ret {
            return_revert!() if self.pending_before.is_some() => {
                self.pending_before = None;
                return (ret, remaining_gas, out);
            }
            _ => (),
        }

        self.validate_before_message();

        let safe_ret = if ret == InstructionResult::CallTooDeep
            || ret == InstructionResult::OutOfFund
            || ret == InstructionResult::StateChangeDuringStaticCall
        {
            InstructionResult::Revert
        } else {
            ret
        };

        let result = match safe_ret.into() {
            SuccessOrHalt::Success(reason) => ExecutionResult::Success {
                reason,
                gas_used: remaining_gas.spend(),
                gas_refunded: remaining_gas.refunded() as u64,
                logs: data.journaled_state.logs.clone(),
                output: Output::Call(out.clone()),
            },
            SuccessOrHalt::Revert => ExecutionResult::Revert {
                gas_used: remaining_gas.spend(),
                output: out.clone(),
            },
            SuccessOrHalt::Halt(reason) => ExecutionResult::Halt {
                reason,
                gas_used: remaining_gas.limit(),
            },
            SuccessOrHalt::Internal => panic!("Internal error: {:?}", safe_ret),
            SuccessOrHalt::FatalExternalError => panic!("Fatal external error"),
        };

        self.trace.add_after(result);

        (ret, remaining_gas, out)
    }

    fn create(
        &mut self,
        data: &mut EVMData<'_, D>,
        inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<rethnet_eth::B160>, Gas, Bytes) {
        self.validate_before_message();

        self.pending_before = Some(BeforeMessage {
            depth: data.journaled_state.depth,
            to: None,
            data: inputs.init_code.clone(),
            value: inputs.value,
            code_address: None,
            code: None,
        });

        (
            InstructionResult::Continue,
            None,
            Gas::new(0),
            Bytes::default(),
        )
    }

    fn create_end(
        &mut self,
        data: &mut EVMData<'_, D>,
        _inputs: &CreateInputs,
        ret: InstructionResult,
        address: Option<rethnet_eth::B160>,
        remaining_gas: Gas,
        out: Bytes,
    ) -> (InstructionResult, Option<rethnet_eth::B160>, Gas, Bytes) {
        self.validate_before_message();

        let safe_ret =
            if ret == InstructionResult::CallTooDeep || ret == InstructionResult::OutOfFund {
                InstructionResult::Revert
            } else {
                ret
            };

        let result = match safe_ret.into() {
            SuccessOrHalt::Success(reason) => ExecutionResult::Success {
                reason,
                gas_used: remaining_gas.spend(),
                gas_refunded: remaining_gas.refunded() as u64,
                logs: data.journaled_state.logs.clone(),
                output: Output::Create(out.clone(), address),
            },
            SuccessOrHalt::Revert => ExecutionResult::Revert {
                gas_used: remaining_gas.spend(),
                output: out.clone(),
            },
            SuccessOrHalt::Halt(reason) => ExecutionResult::Halt {
                reason,
                gas_used: remaining_gas.limit(),
            },
            SuccessOrHalt::Internal => panic!("Internal error: {:?}", safe_ret),
            SuccessOrHalt::FatalExternalError => panic!("Fatal external error"),
        };

        self.trace.add_after(result);

        (ret, address, remaining_gas, out)
    }

    fn step(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, D>,
        _is_static: bool,
    ) -> InstructionResult {
        // Skip the step
        let skip_step = self.pending_before.as_ref().map_or(false, |message| {
            message.code.is_some() && interp.current_opcode() == opcode::STOP
        });

        self.validate_before_message();

        if !skip_step {
            self.trace.add_step(
                data.journaled_state.depth(),
                interp.program_counter(),
                interp.current_opcode(),
                interp.gas(),
                &data.journaled_state.account(interp.contract().address).info,
                &interp.contract().address,
            );
        }

        InstructionResult::Continue
    }
}