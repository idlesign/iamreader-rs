use super::FileInfo;

/// Put the newest fragment first and assign the displayed one-based numbers.
pub(super) fn reverse_and_reindex_file_list(mut file_list: Vec<FileInfo>) -> Vec<FileInfo> {
    file_list.reverse();
    let total_files = file_list.len();

    for (index, file_info) in file_list.iter_mut().enumerate() {
        file_info.index = (total_files - index) as i32;
    }

    file_list
}
