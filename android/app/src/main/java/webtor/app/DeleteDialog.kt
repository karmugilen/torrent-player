package webtor.app

import androidx.compose.foundation.layout.Row
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable

@Composable
fun DeleteDialog(
    title: String,
    onKeepFiles: () -> Unit,
    onEraseFiles: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = MaterialTheme.colorScheme.surface,
        title = { Text("Remove this download?", style = MaterialTheme.typography.titleLarge) },
        text = {
            Text("“${if (title.length > 80) title.take(80) + "…" else title}” leaves the library. Keep its files on the phone, or delete them too.")
        },
        confirmButton = {
            TextButton(onClick = onEraseFiles) { Text("Delete files", color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = {
            Row {
                TextButton(onClick = onDismiss) { Text("Cancel") }
                TextButton(onClick = onKeepFiles) { Text("Keep files") }
            }
        },
    )
}

@Composable
fun ClearAllDialog(
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = MaterialTheme.colorScheme.surface,
        title = { Text("Delete downloaded files?", style = MaterialTheme.typography.titleLarge) },
        text = { Text("Removes files Torrent Player saved in Downloads/Webtor. Library rows stay.") },
        confirmButton = {
            TextButton(onClick = onConfirm) { Text("Delete") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}
