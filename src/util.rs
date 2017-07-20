use kaeru::Metadata;
use shout::ShoutMetadata;

pub fn conv_metadata(md: &Metadata) -> ShoutMetadata {
    let mut smd = ShoutMetadata::new();
   // TODO: Use CStr for Shout
   if let Some(ref title) = md.title {
       smd.add("Title".to_owned(), title.clone());
   }
   if let Some(ref artist) = md.artist {
       smd.add("Artist".to_owned(), artist.clone());
   }
   if let Some(ref album) = md.album {
       smd.add("Album".to_owned(), album.clone());
   }
   if let Some(ref genre) = md.album {
       smd.add("Genre".to_owned(), genre.clone());
   }
   if let Some(ref track) = md.track {
       smd.add("Track".to_owned(), track.clone());
   }
   smd
}
