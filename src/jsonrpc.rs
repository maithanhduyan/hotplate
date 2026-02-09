//! JSON-RPC 2.0 protocol types

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request
#[derive(Deserialize, Debug, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Check if this is a valid JSON-RPC 2.0 request
    pub fn is_valid(&self) -> bool {
        self.jsonrpc == "2.0"
    }

    /// Check if this is a notification (no id)
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 Success Response
#[derive(Serialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

impl JsonRpcResponse {
    /// Create a new success response
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        }
    }
}

/// JSON-RPC 2.0 Error Response
#[derive(Serialize, Debug)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: ErrorObject,
}

impl JsonRpcError {
    /// Create a new error response
    pub fn new(id: Value, code: i32, message: String, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            error: ErrorObject {
                code,
                message,
                data,
            },
        }
    }

    /// Create a parse error response
    pub fn parse_error(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32700,
            "Parse error".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create an invalid request error response
    pub fn invalid_request(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32600,
            "Invalid Request".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create a method not found error response
    pub fn method_not_found(id: Value, method: String) -> Self {
        Self::new(
            id,
            -32601,
            "Method not found".to_string(),
            Some(serde_json::json!({"method": method})),
        )
    }

    /// Create an invalid params error response
    pub fn invalid_params(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32602,
            "Invalid params".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create an internal error response
    pub fn internal_error(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32603,
            "Internal error".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }
}

/// JSON-RPC 2.0 Error Object
#[derive(Serialize, Debug)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    /// Create a new error object
    pub fn new(code: i32, message: String, data: Option<Value>) -> Self {
        Self {
            code,
            message,
            data,
        }
    }
}
