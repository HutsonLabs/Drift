use ironrdp_dvc::DrdynvcServer;

/// Attaches an embedder's own dynamic virtual channel(s) to each new connection.
///
/// The server builds a fresh [`DrdynvcServer`] per connection and offers its built-in channels
/// (input, display control, graphics, ...). A factory registered with
/// [`RdpServer::add_dvc_factory`](crate::RdpServer::add_dvc_factory) is called once per
/// connection to add more, for example a vendor extension channel.
pub trait DvcServerFactory: Send {
    /// Returns `dvc` with this factory's channel processor(s) attached
    /// (`dvc.with_dynamic_channel(...)`).
    fn attach(&self, dvc: DrdynvcServer) -> DrdynvcServer;
}
