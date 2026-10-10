package dev.loams.app.ui.status

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.loams.app.backend.Backend
import dev.loams.app.backend.InstanceSummary
import dev.loams.core.watch.StreamStatus
import dev.loams.proto.loams.operations.v1.Operation
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** The instance and its operations, live over `WatchOperations` (a Connect server stream). */
class StatusViewModel(private val backend: Backend) : ViewModel() {
    val operations: StateFlow<List<Operation>> = backend.operations
    val status: StateFlow<StreamStatus> = backend.operationsStatus

    private val _instance = MutableStateFlow<InstanceSummary?>(null)
    val instance: StateFlow<InstanceSummary?> = _instance.asStateFlow()

    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error.asStateFlow()

    fun refresh() {
        viewModelScope.launch {
            runCatching { backend.instance() }
                .onSuccess { _instance.value = it; _error.value = null }
                .onFailure { _error.value = it.message ?: it.javaClass.simpleName }
        }
    }

    suspend fun watch() = backend.watchOperations()

    fun cancel(op: Operation) {
        viewModelScope.launch { _error.value = backend.cancel(op.id, UUID.randomUUID().toString()) }
    }
}
